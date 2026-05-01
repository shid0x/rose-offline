use std::sync::Arc;

use bevy::prelude::Resource;
use rose_file_readers::VirtualFilesystem;

#[derive(Resource)]
pub struct GameDataVfs {
    pub vfs: Arc<VirtualFilesystem>,
}

impl GameDataVfs {
    pub fn new(vfs: Arc<VirtualFilesystem>) -> Self {
        Self { vfs }
    }
}
