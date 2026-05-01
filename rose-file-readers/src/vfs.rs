use anyhow::{anyhow, Context};
use chrono::Local;
use encoding_rs::EUC_KR;
use memmap::{Mmap, MmapOptions};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    path::{Path, PathBuf},
    sync::RwLock,
};

use crate::{
    reader::RoseFileReader, RoseFileWriter, VfsError, VfsFile, VfsPath, VfsPathBuf,
    VirtualFilesystemDevice,
};

#[derive(Clone)]
struct FileEntry {
    path: PathBuf,
    offset: usize,
    size: usize,
    block_size: u32,
    is_deleted: u8,
    is_compressed: u8,
    is_encrypted: u8,
    version: u32,
    crc: u32,
}

struct Storage {
    filename: String,
    mmap: Mmap,
    entries: Vec<FileEntry>,
    files: HashMap<PathBuf, usize>,
}

#[derive(Default)]
pub struct VfsIndex {
    index_path: PathBuf,
    pub base_version: u32,
    pub current_version: u32,
    storages: RwLock<Vec<Storage>>,
}

pub struct VfsSaveResult {
    pub backups: Vec<PathBuf>,
    pub touched_storage_files: Vec<PathBuf>,
}

impl VfsIndex {
    pub fn load(index_path: &Path) -> Result<VfsIndex, anyhow::Error> {
        let index_root_path = index_path
            .parent()
            .map(|path| path.into())
            .unwrap_or_else(PathBuf::new);
        let data = std::fs::read(index_path)?;
        let mut reader = RoseFileReader::from(&data);

        let base_version = reader.read_u32()?;
        let current_version = reader.read_u32()?;

        let num_vfs = reader.read_u32()? as usize;
        let mut storages = Vec::with_capacity(num_vfs);
        for _ in 0..num_vfs {
            let filename = decode_vfs_string(reader.read_u16_length_bytes()?);
            let offset = reader.read_u32()? as u64;

            let next_vfs_position = reader.position();
            reader.set_position(offset);

            let num_files = reader.read_u32()? as usize;
            let _ = reader.read_u32()?;
            let _ = reader.read_u32()?;

            if filename.eq_ignore_ascii_case("ROOT.VFS") {
                reader.set_position(next_vfs_position);
                continue;
            }

            let file = File::open(index_root_path.join(&filename))?;
            let mmap = unsafe { MmapOptions::new().map(&file)? };

            let mut storage = Storage {
                filename,
                mmap,
                entries: Vec::with_capacity(num_files),
                files: HashMap::with_capacity(num_files),
            };

            for _ in 0..num_files {
                let filename = decode_vfs_string(reader.read_u16_length_bytes()?);
                let offset = reader.read_u32()? as usize;
                let size = reader.read_u32()? as usize;
                let block_size = reader.read_u32()?;
                let is_deleted = reader.read_u8()?;
                let is_compressed = reader.read_u8()?;
                let is_encrypted = reader.read_u8()?;
                let version = reader.read_u32()?;
                let crc = reader.read_u32()?;

                let path = VfsPath::normalise_path(&filename);
                let entry_index = storage.entries.len();
                storage.entries.push(FileEntry {
                    path: path.clone(),
                    offset,
                    size,
                    block_size,
                    is_deleted,
                    is_compressed,
                    is_encrypted,
                    version,
                    crc,
                });

                if is_deleted == 0 {
                    storage.files.insert(path, entry_index);
                }
            }

            storages.push(storage);
            reader.set_position(next_vfs_position);
        }

        Ok(VfsIndex {
            index_path: index_path.to_path_buf(),
            base_version,
            current_version,
            storages: RwLock::new(storages),
        })
    }

    pub fn rewrite_files(
        &self,
        index_path: &Path,
        replacements: &HashMap<VfsPathBuf, Vec<u8>>,
    ) -> Result<VfsSaveResult, anyhow::Error> {
        self.rewrite_files_impl(index_path, replacements, true)
    }

    pub fn rewrite_files_without_backups(
        &self,
        index_path: &Path,
        replacements: &HashMap<VfsPathBuf, Vec<u8>>,
    ) -> Result<VfsSaveResult, anyhow::Error> {
        self.rewrite_files_impl(index_path, replacements, false)
    }

    fn rewrite_files_impl(
        &self,
        index_path: &Path,
        replacements: &HashMap<VfsPathBuf, Vec<u8>>,
        create_backups: bool,
    ) -> Result<VfsSaveResult, anyhow::Error> {
        if replacements.is_empty() {
            return Ok(VfsSaveResult {
                backups: Vec::new(),
                touched_storage_files: Vec::new(),
            });
        }

        let root_path = index_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(PathBuf::new);
        let storages = self
            .storages
            .read()
            .map_err(|_| anyhow!("VFS storage lock is poisoned"))?;

        let normalized_replacements: HashMap<PathBuf, &[u8]> = replacements
            .iter()
            .map(|(path, bytes)| (path.path().to_path_buf(), bytes.as_slice()))
            .collect();

        let mut touched_storage_indices = HashSet::new();
        for path in normalized_replacements.keys() {
            let mut found = false;
            for (storage_index, storage) in storages.iter().enumerate() {
                if storage.files.contains_key(path) {
                    touched_storage_indices.insert(storage_index);
                    found = true;
                    break;
                }
            }

            if !found {
                return Err(anyhow!(
                    "Cannot replace missing VFS file {}",
                    path.to_string_lossy()
                ));
            }
        }

        let mut storage_updates = HashMap::new();
        let mut touched_storage_files = Vec::new();
        for &storage_index in &touched_storage_indices {
            let storage = &storages[storage_index];
            let (bytes, entries) = build_storage_bytes(storage, &normalized_replacements)?;
            storage_updates.insert(storage_index, (bytes, entries));
            touched_storage_files.push(root_path.join(&storage.filename));
        }

        let new_index_bytes = build_index_bytes(self, &storages, &storage_updates)?;

        let timestamp = Local::now().format("%Y%m%d%H%M%S%3f").to_string();
        let mut backups = Vec::new();
        if create_backups {
            backups.push(create_backup(index_path, &timestamp)?);
            for storage_path in &touched_storage_files {
                backups.push(create_backup(storage_path, &timestamp)?);
            }
        }

        let mut temp_storage_paths = Vec::new();
        for (&storage_index, (bytes, _)) in &storage_updates {
            let target_path = root_path.join(&storages[storage_index].filename);
            let temp_path = temp_file_path(&target_path, &timestamp);
            std::fs::write(&temp_path, bytes)?;
            temp_storage_paths.push((target_path, temp_path));
        }

        let temp_index_path = temp_file_path(index_path, &timestamp);
        std::fs::write(&temp_index_path, new_index_bytes)?;

        for (target_path, temp_path) in &temp_storage_paths {
            replace_file_atomically(target_path, temp_path)?;
        }
        replace_file_atomically(index_path, &temp_index_path)?;

        Ok(VfsSaveResult {
            backups,
            touched_storage_files,
        })
    }
}

impl VirtualFilesystemDevice for VfsIndex {
    fn refresh(&self) -> Result<(), anyhow::Error> {
        if self.index_path.as_os_str().is_empty() {
            return Ok(());
        }

        let refreshed_index = VfsIndex::load(&self.index_path)?;
        let refreshed_storages = refreshed_index
            .storages
            .into_inner()
            .map_err(|_| anyhow!("VFS storage lock is poisoned"))?;
        *self
            .storages
            .write()
            .map_err(|_| anyhow!("VFS storage lock is poisoned"))? = refreshed_storages;

        Ok(())
    }

    fn open_file(&self, vfs_path: &VfsPath) -> Result<VfsFile<'_>, anyhow::Error> {
        let storages = self
            .storages
            .read()
            .map_err(|_| anyhow!("VFS storage lock is poisoned"))?;
        for storage in storages.iter() {
            if let Some(entry_index) = storage.files.get(vfs_path.path()) {
                let entry = &storage.entries[*entry_index];
                return Ok(VfsFile::Buffer(
                    storage.mmap[entry.offset..entry.offset + entry.size].to_vec(),
                ));
            }
        }

        Err(VfsError::FileNotFound(vfs_path.path().into()).into())
    }

    fn exists(&self, vfs_path: &VfsPath) -> bool {
        let Ok(storages) = self.storages.read() else {
            return false;
        };
        for storage in storages.iter() {
            if storage.files.get(vfs_path.path()).is_some() {
                return true;
            }
        }

        false
    }

    fn write_file(&self, vfs_path: &VfsPath, data: &[u8]) -> Result<(), anyhow::Error> {
        if self.index_path.as_os_str().is_empty() {
            return Err(anyhow!("Cannot write VFS file without an index path"));
        }

        let current_index = VfsIndex::load(&self.index_path)?;
        let replacements = HashMap::from([(VfsPathBuf::from(vfs_path), data.to_vec())]);
        current_index.rewrite_files_without_backups(&self.index_path, &replacements)?;

        self.refresh()?;
        Ok(())
    }

    fn backup_file(&self, _vfs_path: &VfsPath) -> Result<(), anyhow::Error> {
        // VfsIndex::rewrite_files creates timestamped backups for data.idx and every touched
        // storage file as part of the atomic rewrite.
        Ok(())
    }
}

fn decode_vfs_string(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    let (decoded, _, _) = EUC_KR.decode(bytes);
    decoded.into_owned()
}

fn write_vfs_string(writer: &mut RoseFileWriter, value: &str) {
    let (encoded, _, _) = EUC_KR.encode(value);
    let mut bytes = encoded.into_owned();
    bytes.push(0);
    writer.write_u16_length_bytes(&bytes);
}

fn format_vfs_path(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\")
}

fn build_storage_bytes(
    storage: &Storage,
    replacements: &HashMap<PathBuf, &[u8]>,
) -> Result<(Vec<u8>, Vec<FileEntry>), anyhow::Error> {
    let mut bytes = Vec::new();
    let mut entries = Vec::with_capacity(storage.entries.len());

    for entry in &storage.entries {
        let mut updated_entry = entry.clone();
        if entry.is_deleted != 0 {
            updated_entry.offset = 0;
            updated_entry.size = 0;
            updated_entry.block_size = 0;
        } else {
            let file_bytes = if let Some(replacement) = replacements.get(&entry.path) {
                *replacement
            } else {
                storage
                    .mmap
                    .get(entry.offset..entry.offset + entry.size)
                    .ok_or_else(|| {
                        anyhow!("Invalid VFS entry bounds for {}", entry.path.display())
                    })?
            };

            updated_entry.offset = bytes.len();
            updated_entry.size = file_bytes.len();
            updated_entry.block_size = file_bytes.len() as u32;
            bytes.extend_from_slice(file_bytes);
        }

        entries.push(updated_entry);
    }

    Ok((bytes, entries))
}

fn build_index_bytes(
    index: &VfsIndex,
    storages: &[Storage],
    storage_updates: &HashMap<usize, (Vec<u8>, Vec<FileEntry>)>,
) -> Result<Vec<u8>, anyhow::Error> {
    let mut writer = RoseFileWriter::default();
    writer.write_u32(index.base_version);
    writer.write_u32(index.current_version);
    writer.write_u32((storages.len() + 1) as u32);

    let mut offset_slots = Vec::with_capacity(storages.len() + 1);
    write_vfs_string(&mut writer, "ROOT.VFS");
    offset_slots.push(writer.buffer.len());
    writer.write_u32(0);

    for storage in storages {
        write_vfs_string(&mut writer, &storage.filename);
        offset_slots.push(writer.buffer.len());
        writer.write_u32(0);
    }

    let root_offset = writer.buffer.len() as u32;
    writer.write_u32(0);
    writer.write_u32(0);
    writer.write_u32(0);

    writer.buffer[offset_slots[0]..offset_slots[0] + 4].copy_from_slice(&root_offset.to_le_bytes());

    for (storage_index, storage) in storages.iter().enumerate() {
        let metadata_offset = writer.buffer.len() as u32;
        writer.buffer[offset_slots[storage_index + 1]..offset_slots[storage_index + 1] + 4]
            .copy_from_slice(&metadata_offset.to_le_bytes());

        let entries = storage_updates
            .get(&storage_index)
            .map(|(_, entries)| entries.as_slice())
            .unwrap_or_else(|| storage.entries.as_slice());

        writer.write_u32(entries.len() as u32);
        writer.write_u32(0);
        writer.write_u32(0);
        for entry in entries {
            write_vfs_string(&mut writer, &format_vfs_path(&entry.path));
            writer.write_u32(entry.offset as u32);
            writer.write_u32(entry.size as u32);
            writer.write_u32(entry.block_size);
            writer.write_u8(entry.is_deleted);
            writer.write_u8(entry.is_compressed);
            writer.write_u8(entry.is_encrypted);
            writer.write_u32(entry.version);
            writer.write_u32(entry.crc);
        }
    }

    Ok(writer.buffer.to_vec())
}

fn temp_file_path(path: &Path, timestamp: &str) -> PathBuf {
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("temp"));
    path.with_file_name(format!("{}.{}.tmp", filename, timestamp))
}

fn create_backup(path: &Path, timestamp: &str) -> Result<PathBuf, anyhow::Error> {
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| String::from("backup"));
    let backup_path = path.with_file_name(format!("{}.{}.bak", filename, timestamp));
    std::fs::copy(path, &backup_path).with_context(|| {
        format!(
            "Failed to create backup {} from {}",
            backup_path.display(),
            path.display()
        )
    })?;
    Ok(backup_path)
}

fn replace_file_atomically(target_path: &Path, temp_path: &Path) -> Result<(), anyhow::Error> {
    if target_path.exists() {
        std::fs::remove_file(target_path)
            .with_context(|| format!("Failed to remove {}", target_path.display()))?;
    }
    std::fs::rename(temp_path, target_path).with_context(|| {
        format!(
            "Failed to move {} into place at {}",
            temp_path.display(),
            target_path.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{decode_vfs_string, write_vfs_string, VfsIndex};
    use crate::{RoseFileReader, RoseFileWriter, VfsFile, VfsPathBuf, VirtualFilesystemDevice};
    use std::{collections::HashMap, path::Path};
    use tempfile::tempdir;

    fn file_bytes(file: VfsFile<'_>) -> Vec<u8> {
        match file {
            VfsFile::Buffer(bytes) => bytes,
            VfsFile::View(bytes) => bytes.to_vec(),
        }
    }

    fn create_test_vfs(root: &Path) {
        let storage_bytes = b"HELLOWORLD".to_vec();
        std::fs::write(root.join("DATA.VFS"), &storage_bytes).unwrap();

        let mut writer = RoseFileWriter::default();
        writer.write_u32(1);
        writer.write_u32(1);
        writer.write_u32(2);

        write_vfs_string(&mut writer, "ROOT.VFS");
        let root_offset_slot = writer.buffer.len();
        writer.write_u32(0);

        write_vfs_string(&mut writer, "DATA.VFS");
        let storage_offset_slot = writer.buffer.len();
        writer.write_u32(0);

        let root_offset = writer.buffer.len() as u32;
        writer.write_u32(0);
        writer.write_u32(0);
        writer.write_u32(0);
        writer.buffer[root_offset_slot..root_offset_slot + 4]
            .copy_from_slice(&root_offset.to_le_bytes());

        let storage_offset = writer.buffer.len() as u32;
        writer.buffer[storage_offset_slot..storage_offset_slot + 4]
            .copy_from_slice(&storage_offset.to_le_bytes());
        writer.write_u32(2);
        writer.write_u32(0);
        writer.write_u32(0);

        write_vfs_string(&mut writer, "3DDATA/STB/LIST_NPC.STB");
        writer.write_u32(0);
        writer.write_u32(5);
        writer.write_u32(5);
        writer.write_u8(0);
        writer.write_u8(0);
        writer.write_u8(0);
        writer.write_u32(1);
        writer.write_u32(11);

        write_vfs_string(&mut writer, "3DDATA/STB/LIST_SELL.STB");
        writer.write_u32(5);
        writer.write_u32(5);
        writer.write_u32(5);
        writer.write_u8(0);
        writer.write_u8(0);
        writer.write_u8(0);
        writer.write_u32(1);
        writer.write_u32(22);

        std::fs::write(root.join("data.idx"), writer.buffer.as_ref()).unwrap();
    }

    #[test]
    fn decode_and_encode_vfs_string_round_trip() {
        let mut writer = RoseFileWriter::default();
        write_vfs_string(&mut writer, "3DDATA/STB/LIST_NPC.STB");
        let mut reader = RoseFileReader::from(writer.buffer.as_ref());
        let value = decode_vfs_string(reader.read_u16_length_bytes().unwrap());
        assert_eq!(value, "3DDATA/STB/LIST_NPC.STB");
    }

    #[test]
    fn rewrite_files_updates_storage_and_creates_backups() {
        let dir = tempdir().unwrap();
        create_test_vfs(dir.path());

        let index_path = dir.path().join("data.idx");
        let vfs = VfsIndex::load(&index_path).unwrap();
        assert_eq!(
            file_bytes(vfs.open_file(&"3DDATA/STB/LIST_NPC.STB".into()).unwrap()),
            b"HELLO"
        );

        let mut replacements = HashMap::new();
        replacements.insert(
            VfsPathBuf::new("3DDATA/STB/LIST_SELL.STB"),
            b"STORE!".to_vec(),
        );

        let result = vfs.rewrite_files(&index_path, &replacements).unwrap();
        assert_eq!(result.touched_storage_files.len(), 1);
        assert_eq!(result.backups.len(), 2);

        let reloaded = VfsIndex::load(&index_path).unwrap();
        assert_eq!(
            file_bytes(
                reloaded
                    .open_file(&"3DDATA/STB/LIST_NPC.STB".into())
                    .unwrap()
            ),
            b"HELLO"
        );
        assert_eq!(
            file_bytes(
                reloaded
                    .open_file(&"3DDATA/STB/LIST_SELL.STB".into())
                    .unwrap()
            ),
            b"STORE!"
        );
    }

    #[test]
    fn refresh_sees_external_vfs_rewrite() {
        let dir = tempdir().unwrap();
        create_test_vfs(dir.path());

        let index_path = dir.path().join("data.idx");
        let stale_vfs = VfsIndex::load(&index_path).unwrap();
        assert_eq!(
            file_bytes(
                stale_vfs
                    .open_file(&"3DDATA/STB/LIST_SELL.STB".into())
                    .unwrap()
            ),
            b"WORLD"
        );

        let writer_vfs = VfsIndex::load(&index_path).unwrap();
        let replacements = HashMap::from([(
            VfsPathBuf::new("3DDATA/STB/LIST_SELL.STB"),
            b"STORE!".to_vec(),
        )]);
        writer_vfs
            .rewrite_files_without_backups(&index_path, &replacements)
            .unwrap();

        assert_eq!(
            file_bytes(
                stale_vfs
                    .open_file(&"3DDATA/STB/LIST_SELL.STB".into())
                    .unwrap()
            ),
            b"WORLD"
        );

        stale_vfs.refresh().unwrap();

        assert_eq!(
            file_bytes(
                stale_vfs
                    .open_file(&"3DDATA/STB/LIST_SELL.STB".into())
                    .unwrap()
            ),
            b"STORE!"
        );
    }

    #[test]
    fn rewrite_files_preserves_backslash_vfs_paths() {
        let dir = tempdir().unwrap();
        create_test_vfs(dir.path());

        let index_path = dir.path().join("data.idx");
        let vfs = VfsIndex::load(&index_path).unwrap();
        let mut replacements = HashMap::new();
        replacements.insert(
            VfsPathBuf::new("3DDATA/STB/LIST_NPC.STB"),
            b"HELLO!".to_vec(),
        );

        vfs.rewrite_files(&index_path, &replacements).unwrap();

        let data = std::fs::read(&index_path).unwrap();
        let mut reader = RoseFileReader::from(data.as_slice());
        reader.read_u32().unwrap();
        reader.read_u32().unwrap();
        reader.read_u32().unwrap();

        let _root_name = decode_vfs_string(reader.read_u16_length_bytes().unwrap());
        let _root_offset = reader.read_u32().unwrap();
        let _storage_name = decode_vfs_string(reader.read_u16_length_bytes().unwrap());
        let storage_offset = reader.read_u32().unwrap() as u64;

        reader.set_position(storage_offset);
        let entry_count = reader.read_u32().unwrap();
        assert_eq!(entry_count, 2);
        reader.read_u32().unwrap();
        reader.read_u32().unwrap();

        let first_path = decode_vfs_string(reader.read_u16_length_bytes().unwrap());
        assert_eq!(first_path, "3DDATA\\STB\\LIST_NPC.STB");
    }
}
