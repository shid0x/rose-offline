use log::{debug, warn};
use std::{collections::HashMap, sync::Arc};

use rose_data::{QuestData, QuestDatabase, StringDatabase, WorldTicks};
use rose_file_readers::{stb_column, QsdFile, StbFile, StbReadOptions, VirtualFilesystem};

struct StbQuest(StbFile);

impl StbQuest {
    stb_column! { 1, get_time_limit, WorldTicks }
    stb_column! { 3, get_icon_id, u32 }
}

pub fn get_quest_database(
    vfs: &VirtualFilesystem,
    string_database: Arc<StringDatabase>,
) -> Result<QuestDatabase, anyhow::Error> {
    let quest_s_stb = vfs.read_file_with::<StbFile, _>(
        "3DDATA/QUESTDATA/QUEST_S.STB",
        &StbReadOptions {
            is_wide: true,
            ..Default::default()
        },
    )?;
    let mut strings = HashMap::new();

    for row in 0..quest_s_stb.rows() {
        let english = quest_s_stb.get(row, 1);
        if !english.is_empty() {
            strings.insert(row as u16, english.to_string());
        }
    }

    let quest_stb = StbQuest(vfs.read_file::<StbFile, _>("3DDATA/STB/LIST_QUEST.STB")?);
    let mut quests = Vec::new();
    for row in 0..quest_stb.0.rows() {
        let time_limit = quest_stb.get_time_limit(row).filter(|x| x.0 != 0);
        let string_id = quest_stb.0.try_get(row, quest_stb.0.columns() - 1);

        if let Some(string_id) = string_id {
            let quest_strings = string_database.get_quest(string_id);
            quests.push(Some(QuestData {
                id: row,
                icon_id: quest_stb.get_icon_id(row).unwrap_or(0),
                name: quest_strings
                    .as_ref()
                    .map_or("", |x| unsafe { std::mem::transmute(x.name) }),
                description: quest_strings
                    .as_ref()
                    .map_or("", |x| unsafe { std::mem::transmute(x.description) }),
                start_message: quest_strings
                    .as_ref()
                    .map_or("", |x| unsafe { std::mem::transmute(x.start_message) }),
                end_message: quest_strings
                    .as_ref()
                    .map_or("", |x| unsafe { std::mem::transmute(x.end_message) }),
                time_limit,
            }));
        } else {
            quests.push(None);
        }
    }

    let qsd_files_stb = vfs.read_file::<StbFile, _>("3DDATA/STB/LIST_QUESTDATA.STB")?;
    let mut triggers = HashMap::new();

    for row in 0..qsd_files_stb.rows() {
        let qsd_path = qsd_files_stb.get(row, 0);
        if qsd_path.is_empty() {
            continue;
        }

        match vfs.read_file::<QsdFile, _>(qsd_path) {
            Ok(qsd) => triggers.extend(qsd.triggers),
            Err(error) => warn!("Failed to parse {}, error: {:?}", qsd_path, error),
        }
    }

    let mut triggers_by_hash = HashMap::new();
    for key in triggers.keys() {
        triggers_by_hash.insert(key.as_str().into(), key.clone());
    }

    debug!("Loaded {} QSD triggers", triggers.len());
    Ok(QuestDatabase {
        _string_database: string_database,
        quests,
        strings,
        triggers,
        triggers_by_hash,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rose_data::QuestTriggerHash;
    use rose_file_readers::{HostFilesystemDevice, StbFile, VirtualFilesystem};

    use crate::get_string_database;

    use super::get_quest_database;

    fn create_test_vfs() -> VirtualFilesystem {
        let root_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        VirtualFilesystem::new(vec![Box::new(HostFilesystemDevice::new(root_path))])
    }

    #[test]
    fn test_quest_database_resolves_pvp1301_exit_trigger_by_hash() {
        let vfs = create_test_vfs();
        let string_database =
            get_string_database(&vfs, 1).expect("failed to load string database for test");
        let quest_database =
            get_quest_database(&vfs, string_database).expect("failed to load quest database");

        let trigger = quest_database
            .get_trigger_by_hash(QuestTriggerHash::from("PvP1301-303"))
            .expect("missing PvP1301-303 in quest database");

        assert_eq!(trigger.name, "PvP1301-303");
    }

    #[test]
    fn test_quest_database_loads_icon_id_from_list_quest_stb() {
        let vfs = create_test_vfs();
        let string_database =
            get_string_database(&vfs, 1).expect("failed to load string database for test");
        let quest_database =
            get_quest_database(&vfs, string_database).expect("failed to load quest database");
        let raw_quest_stb = vfs
            .read_file::<StbFile, _>("3DDATA/STB/LIST_QUEST.STB")
            .expect("failed to load raw LIST_QUEST.STB");

        let quest_id = (0..raw_quest_stb.rows())
            .find(|&row| raw_quest_stb.get(row, raw_quest_stb.columns() - 1) != "")
            .expect("expected at least one quest row");
        let expected_icon_id = raw_quest_stb
            .try_get(quest_id, 3)
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);

        let quest_data = quest_database
            .get_quest_data(quest_id)
            .expect("missing quest data for selected row");

        assert_eq!(quest_data.icon_id, expected_icon_id);
    }
}
