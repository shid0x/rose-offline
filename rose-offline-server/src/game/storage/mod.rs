use std::path::PathBuf;

use directories::ProjectDirs;
use lazy_static::lazy_static;

fn local_storage_dir() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = std::env::var_os("ROSE_OFFLINE_TEST_DATA_DIR") {
        return PathBuf::from(path);
    }

    let project = ProjectDirs::from("", "", "rose-offline").unwrap();
    PathBuf::from(project.data_local_dir())
}

lazy_static! {
    pub static ref LOCAL_STORAGE_DIR: PathBuf = local_storage_dir();
    pub static ref ACCOUNT_STORAGE_DIR: PathBuf = LOCAL_STORAGE_DIR.join("accounts");
    pub static ref BANK_STORAGE_DIR: PathBuf = LOCAL_STORAGE_DIR.join("bank");
    pub static ref CHARACTER_STORAGE_DIR: PathBuf = LOCAL_STORAGE_DIR.join("characters");
    pub static ref CLAN_STORAGE_DIR: PathBuf = LOCAL_STORAGE_DIR.join("clan");
}

pub mod account;
pub mod bank;
pub mod character;
pub mod clan;
