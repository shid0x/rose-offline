use bevy::prelude::Resource;
use rose_game_common::components::{get_default_clan_upgrade_points, ClanPoints, MAX_CLAN_LEVEL};

#[derive(Resource)]
pub struct GameConfig {
    pub enable_npc_spawns: bool,
    pub enable_monster_spawns: bool,
    pub clan_upgrade_requirements: [u64; (MAX_CLAN_LEVEL as usize) + 1],
}

impl GameConfig {
    pub fn default() -> Self {
        let mut clan_upgrade_requirements = [0; (MAX_CLAN_LEVEL as usize) + 1];
        for next_level in 2..=MAX_CLAN_LEVEL {
            clan_upgrade_requirements[next_level as usize] =
                get_default_clan_upgrade_points(next_level).map_or(0, |points| points.0);
        }

        Self {
            enable_monster_spawns: true,
            enable_npc_spawns: true,
            clan_upgrade_requirements,
        }
    }

    pub fn clan_upgrade_points_required(&self, next_level: u32) -> Option<ClanPoints> {
        if !(2..=MAX_CLAN_LEVEL).contains(&next_level) {
            return None;
        }

        Some(ClanPoints(
            self.clan_upgrade_requirements[next_level as usize],
        ))
    }
}
