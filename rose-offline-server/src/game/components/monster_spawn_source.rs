use bevy::ecs::prelude::Component;
use rose_data::ZoneId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Component)]
pub struct MonsterSpawnSource {
    pub zone_id: ZoneId,
    pub block_x: u32,
    pub block_y: u32,
    pub spawn_index: usize,
}

impl MonsterSpawnSource {
    pub fn new(zone_id: ZoneId, block_x: u32, block_y: u32, spawn_index: usize) -> Self {
        Self {
            zone_id,
            block_x,
            block_y,
            spawn_index,
        }
    }
}
