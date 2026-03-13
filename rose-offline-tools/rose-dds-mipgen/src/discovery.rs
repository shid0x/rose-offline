use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use rose_file_readers::{
    ChrFile, EftFile, IfoFile, LitFile, PtlFile, StbFile, VfsPath, VfsPathBuf, VirtualFilesystem,
    ZonFile, ZscFile,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryScope {
    ZscOnly,
    Full,
}

impl DiscoveryScope {
    pub fn from_str(value: &str) -> Self {
        match value {
            "zsc-only" => Self::ZscOnly,
            "full" => Self::Full,
            _ => Self::Full,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ZscOnly => "zsc-only",
            Self::Full => "full",
        }
    }
}

pub fn discover_textures(
    vfs: &VirtualFilesystem,
    scope: DiscoveryScope,
) -> (HashSet<VfsPathBuf>, HashSet<VfsPathBuf>) {
    match scope {
        DiscoveryScope::ZscOnly => discover_textures_zsc_only(vfs),
        DiscoveryScope::Full => discover_textures_full(vfs),
    }
}

fn discover_textures_zsc_only(
    vfs: &VirtualFilesystem,
) -> (HashSet<VfsPathBuf>, HashSet<VfsPathBuf>) {
    let mut textures = HashSet::new();
    let mut zsc_paths = HashSet::new();

    for zsc_path in get_known_zsc_paths() {
        let Ok(zsc) = vfs.read_file::<ZscFile, _>(zsc_path) else {
            continue;
        };

        zsc_paths.insert(VfsPathBuf::new(zsc_path));

        for material in &zsc.materials {
            if is_dds_path(material.path.path()) {
                textures.insert(material.path.clone());
            }
        }
    }

    (textures, zsc_paths)
}

fn discover_textures_full(vfs: &VirtualFilesystem) -> (HashSet<VfsPathBuf>, HashSet<VfsPathBuf>) {
    let mut file_list = FoundFiles::new(vfs);
    let base_file_list = read_base_file_list();

    for file in &base_file_list {
        file_list.try_add_file(file);
    }

    for stb_path in file_list.get_with_extension("STB") {
        let Ok(stb) = vfs.read_file::<StbFile, _>(stb_path.path()) else {
            continue;
        };

        for x in 0..stb.rows() {
            for y in 0..stb.columns() {
                let value = stb.get(x, y);
                if !value.is_empty() {
                    file_list.try_add_file(value);
                }
            }
        }
    }

    for chr_path in file_list.get_with_extension("CHR") {
        let Ok(chr) = vfs.read_file::<ChrFile, _>(chr_path.path()) else {
            continue;
        };

        for skeleton_path in &chr.skeleton_files {
            file_list.try_add_file(skeleton_path);
        }
        for motion_path in &chr.motion_files {
            file_list.try_add_file(motion_path);
        }
        for effect_path in &chr.effect_files {
            file_list.try_add_file(effect_path);
        }
    }

    for zsc_path in file_list.get_with_extension("ZSC") {
        let Ok(zsc) = vfs.read_file::<ZscFile, _>(zsc_path.path()) else {
            continue;
        };

        for mesh_path in &zsc.meshes {
            file_list.try_add_file(mesh_path);
        }
        for material in &zsc.materials {
            file_list.try_add_file(&material.path);
        }
        for effect_path in &zsc.effects {
            file_list.try_add_file(effect_path);
        }
        for object in &zsc.objects {
            for part in &object.parts {
                if let Some(animation_path) = part.animation_path.as_ref() {
                    file_list.try_add_file(animation_path);
                }
            }
        }
    }

    for zon_path in file_list.get_with_extension("ZON") {
        if let Ok(zon) = vfs.read_file::<ZonFile, _>(zon_path.path()) {
            for texture_path in &zon.tile_textures {
                file_list.try_add_file(texture_path);
            }
        }

        let zone_directory = zon_path.path().parent().unwrap_or_else(|| Path::new(""));
        for block_y in 0..64 {
            for block_x in 0..64 {
                file_list.try_add_file(zone_directory.join(format!("{}_{}.HIM", block_x, block_y)));
                file_list.try_add_file(zone_directory.join(format!("{}_{}.TIL", block_x, block_y)));
                file_list.try_add_file(zone_directory.join(format!("{}_{}.IFO", block_x, block_y)));
                file_list.try_add_file(zone_directory.join(format!("{}_{}.MOV", block_x, block_y)));
                file_list.try_add_file(zone_directory.join(format!(
                    "{}_{}/LIGHTMAP/BUILDINGLIGHTMAPDATA.LIT",
                    block_x, block_y
                )));
                file_list.try_add_file(zone_directory.join(format!(
                    "{}_{}/LIGHTMAP/OBJECTLIGHTMAPDATA.LIT",
                    block_x, block_y
                )));
                file_list.try_add_file(zone_directory.join(format!(
                    "{0:}_{1:}/{0:}_{1:}_PLANELIGHTINGMAP.DDS",
                    block_x, block_y
                )));
            }
        }
    }

    for lit_path in file_list.get_with_extension("LIT") {
        let Ok(lit) = vfs.read_file::<LitFile, _>(lit_path.path()) else {
            continue;
        };

        for object in &lit.objects {
            for part in &object.parts {
                file_list.try_add_file(&part.filename);
            }
        }
    }

    for ifo_path in file_list.get_with_extension("IFO") {
        let Ok(ifo) = vfs.read_file::<IfoFile, _>(ifo_path.path()) else {
            continue;
        };

        for effect_object in &ifo.effect_objects {
            file_list.try_add_file(&effect_object.effect_path);
        }

        for sound_object in &ifo.sound_objects {
            file_list.try_add_file(&sound_object.sound_path);
        }

        for npc in &ifo.npcs {
            file_list.try_add_file(&npc.quest_file_name);
        }
    }

    for eft_path in file_list.get_with_extension("EFT") {
        let Ok(eft) = vfs.read_file::<EftFile, _>(eft_path.path()) else {
            continue;
        };

        if let Some(sound_file) = &eft.sound_file {
            file_list.try_add_file(sound_file);
        }

        for particle in &eft.particles {
            file_list.try_add_file(&particle.particle_file);
            if let Some(animation_file) = &particle.animation_file {
                file_list.try_add_file(animation_file);
            }
        }

        for mesh in &eft.meshes {
            file_list.try_add_file(&mesh.mesh_texture_file);
            if let Some(mesh_animation_file) = &mesh.mesh_animation_file {
                file_list.try_add_file(mesh_animation_file);
            }
            if let Some(animation_file) = &mesh.animation_file {
                file_list.try_add_file(animation_file);
            }
        }
    }

    for ptl_path in file_list.get_with_extension("PTL") {
        let Ok(ptl) = vfs.read_file::<PtlFile, _>(ptl_path.path()) else {
            continue;
        };

        for sequence in &ptl.sequences {
            file_list.try_add_file(&sequence.texture_path);
        }
    }

    let zsc_paths: HashSet<VfsPathBuf> = file_list.get_with_extension("ZSC").into_iter().collect();
    let textures: HashSet<VfsPathBuf> = file_list
        .get_with_extension("DDS")
        .into_iter()
        .filter(|path| path.path().starts_with("3DDATA"))
        .collect();

    (textures, zsc_paths)
}

fn read_base_file_list() -> Vec<String> {
    // Keep discovery seeds in sync with rose-vfs-dump without duplicating the list here.
    let source = include_str!("../../rose-vfs-dump/src/main.rs");
    let start = source.find("const BASE_FILE_LIST").unwrap_or(0);
    let slice = &source[start..];
    let end_rel = slice.find("];").unwrap_or(slice.len());
    let block = &slice[..end_rel];

    block
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if !trimmed.starts_with('"') {
                return None;
            }
            let value = trimmed.trim_end_matches(',').trim_matches('"');
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        })
        .collect()
}

fn is_dds_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("dds"))
        .unwrap_or(false)
}

struct FoundFiles<'a> {
    vfs: &'a VirtualFilesystem,
    all_files: HashSet<VfsPathBuf>,
    by_extension: HashMap<String, HashSet<VfsPathBuf>>,
}

impl<'a> FoundFiles<'a> {
    fn new(vfs: &'a VirtualFilesystem) -> Self {
        Self {
            vfs,
            all_files: HashSet::new(),
            by_extension: HashMap::new(),
        }
    }

    fn get_with_extension(&self, extension: &str) -> Vec<VfsPathBuf> {
        let extension = extension.to_ascii_uppercase();
        self.by_extension
            .get(&extension)
            .map_or_else(Vec::new, |list| list.iter().cloned().collect())
    }

    fn try_add_file<'p, P: Into<VfsPath<'p>>>(&mut self, path: P) -> bool {
        let path: VfsPath = path.into();
        if !self.vfs.exists(&path) {
            return false;
        }

        let path: VfsPathBuf = (&path).into();
        if self.all_files.contains(&path) {
            return false;
        }

        let Some(extension) = path.path().extension() else {
            return false;
        };

        let extension = extension.to_string_lossy().to_ascii_uppercase();
        if extension.is_empty() {
            return false;
        }

        self.all_files.insert(path.clone());
        self.by_extension
            .entry(extension)
            .or_insert_with(HashSet::new)
            .insert(path);

        true
    }
}

/// Returns a list of known ZSC file paths in ROSE Online.
fn get_known_zsc_paths() -> Vec<&'static str> {
    vec![
        "3DDATA/AVATAR/LIST_BACK.ZSC",
        "3DDATA/AVATAR/LIST_FACEIEM.ZSC",
        "3DDATA/AVATAR/LIST_MARMS.ZSC",
        "3DDATA/AVATAR/LIST_MBODY.ZSC",
        "3DDATA/AVATAR/LIST_MCAP.ZSC",
        "3DDATA/AVATAR/LIST_MFACE.ZSC",
        "3DDATA/AVATAR/LIST_MFOOT.ZSC",
        "3DDATA/AVATAR/LIST_MHAIR.ZSC",
        "3DDATA/AVATAR/LIST_WARMS.ZSC",
        "3DDATA/AVATAR/LIST_WBODY.ZSC",
        "3DDATA/AVATAR/LIST_WCAP.ZSC",
        "3DDATA/AVATAR/LIST_WFACE.ZSC",
        "3DDATA/AVATAR/LIST_WFOOT.ZSC",
        "3DDATA/AVATAR/LIST_WHAIR.ZSC",
        "3DDATA/WEAPON/LIST_WEAPON.ZSC",
        "3DDATA/WEAPON/LIST_SUBWEAPON.ZSC",
        "3DDATA/NPC/LIST_NPC.ZSC",
        "3DDATA/PAT/LIST_PAT.ZSC",
        "3DDATA/JUNON/LIST_CNST_JPT.ZSC",
        "3DDATA/JUNON/LIST_DECO_JPT.ZSC",
        "3DDATA/LUNAR/LIST_CNST_JPT.ZSC",
        "3DDATA/LUNAR/LIST_DECO_JPT.ZSC",
        "3DDATA/ELDEON/LIST_CNST_JPT.ZSC",
        "3DDATA/ELDEON/LIST_DECO_JPT.ZSC",
        "3DDATA/ORO/LIST_CNST_JPT.ZSC",
        "3DDATA/ORO/LIST_DECO_JPT.ZSC",
        "3DDATA/SPECIAL/EVENT_OBJECT.ZSC",
        "3DDATA/SPECIAL/LIST_DECO_SPECIAL.ZSC",
    ]
}
