use std::collections::VecDeque;
use std::time::Duration;

use bevy::{ecs::prelude::Entity, prelude::Resource, utils::HashSet};
use rose_data::{ZoneId, ZoneMonsterSpawnPoint};

pub const LIVE_SPAWN_RELOAD_DESPAWNS_PER_TICK: usize = 16;
pub const LIVE_SPAWN_RELOAD_STAGGER: Duration = Duration::from_secs(2);

pub struct LiveSpawnReloadJob {
    pub zone_id: ZoneId,
    pub block_x: u32,
    pub block_y: u32,
    pub reloaded_spawns: Vec<ZoneMonsterSpawnPoint>,
    pub old_spawn_entities: HashSet<Entity>,
    pub pending_monsters: VecDeque<Entity>,
}

impl LiveSpawnReloadJob {
    pub fn new(
        zone_id: ZoneId,
        block_x: u32,
        block_y: u32,
        reloaded_spawns: Vec<ZoneMonsterSpawnPoint>,
        old_spawn_entities: HashSet<Entity>,
        pending_monsters: VecDeque<Entity>,
    ) -> Self {
        Self {
            zone_id,
            block_x,
            block_y,
            reloaded_spawns,
            old_spawn_entities,
            pending_monsters,
        }
    }
}

#[derive(Default, Resource)]
pub struct LiveSpawnReloadQueue {
    jobs: VecDeque<LiveSpawnReloadJob>,
}

impl LiveSpawnReloadQueue {
    pub fn push(&mut self, job: LiveSpawnReloadJob) {
        self.jobs.push_back(job);
    }

    pub fn front_mut(&mut self) -> Option<&mut LiveSpawnReloadJob> {
        self.jobs.front_mut()
    }

    pub fn pop_front(&mut self) -> Option<LiveSpawnReloadJob> {
        self.jobs.pop_front()
    }

    pub fn is_spawn_point_blocked(&self, entity: Entity) -> bool {
        self.jobs
            .iter()
            .any(|job| job.old_spawn_entities.contains(&entity))
    }
}
