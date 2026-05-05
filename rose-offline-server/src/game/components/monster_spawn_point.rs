use bevy::ecs::prelude::Component;
use std::time::Duration;

use rose_data::{NpcId, ZoneMonsterSpawnPoint};

#[derive(Component)]
pub struct MonsterSpawnPoint {
    pub basic_spawns: Vec<(NpcId, usize)>,
    pub tactic_spawns: Vec<(NpcId, usize)>,
    pub interval: Duration,
    pub limit_count: u32,
    pub range: u32,
    pub tactic_points: u32,

    pub time_since_last_check: Duration,
    pub current_tactics_value: u32,
    pub num_alive_monsters: u32,
}

impl MonsterSpawnPoint {
    fn interval_from_seconds(interval_seconds: u32) -> Duration {
        Duration::from_secs(interval_seconds as u64)
    }

    pub fn advance_spawn_check(&mut self, delta: Duration) -> bool {
        self.time_since_last_check = self.time_since_last_check.saturating_add(delta);
        if self.time_since_last_check < self.interval {
            return false;
        }

        self.time_since_last_check = self.time_since_last_check.saturating_sub(self.interval);
        true
    }

    pub fn apply_spawn_data(&mut self, spawn_point: &ZoneMonsterSpawnPoint) {
        self.basic_spawns = spawn_point.basic_spawns.clone();
        self.tactic_spawns = spawn_point.tactic_spawns.clone();
        self.interval = Self::interval_from_seconds(spawn_point.interval);
        self.limit_count = spawn_point.limit_count;
        self.range = spawn_point.range;
        self.tactic_points = spawn_point.tactic_points;
        self.time_since_last_check = self.time_since_last_check.min(self.interval);
    }

    pub fn reset_for_live_reload(&mut self) {
        self.reset_for_live_reload_with_delay(Duration::ZERO);
    }

    pub fn reset_for_live_reload_with_delay(&mut self, delay: Duration) {
        self.num_alive_monsters = 0;
        self.current_tactics_value = 1;
        self.time_since_last_check = self.interval.saturating_sub(delay);
    }
}

impl From<&ZoneMonsterSpawnPoint> for MonsterSpawnPoint {
    fn from(spawn_point: &ZoneMonsterSpawnPoint) -> Self {
        let interval = Self::interval_from_seconds(spawn_point.interval);

        Self {
            basic_spawns: spawn_point.basic_spawns.clone(),
            tactic_spawns: spawn_point.tactic_spawns.clone(),
            interval,
            limit_count: spawn_point.limit_count,
            range: spawn_point.range,
            tactic_points: spawn_point.tactic_points,

            time_since_last_check: interval,
            current_tactics_value: 1,
            num_alive_monsters: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::math::Vec3;
    use rose_data::{NpcId, ZoneMonsterSpawnPoint};

    use super::MonsterSpawnPoint;

    fn test_zone_spawn_point(interval: u32) -> ZoneMonsterSpawnPoint {
        ZoneMonsterSpawnPoint {
            source_block_x: 0,
            source_block_y: 0,
            source_spawn_index: 0,
            position: Vec3::ZERO,
            basic_spawns: vec![(NpcId::new(1).unwrap(), 1)],
            tactic_spawns: Vec::new(),
            interval,
            limit_count: 1,
            range: 0,
            tactic_points: 100,
        }
    }

    #[test]
    fn monster_spawn_point_starts_ready_for_first_spawn_check() {
        let spawn_point = MonsterSpawnPoint::from(&test_zone_spawn_point(30));

        assert_eq!(spawn_point.interval, Duration::from_secs(30));
        assert_eq!(spawn_point.time_since_last_check, spawn_point.interval);
        assert_eq!(spawn_point.current_tactics_value, 1);
    }

    #[test]
    fn monster_spawn_point_first_check_is_immediate_then_waits_for_interval() {
        let mut spawn_point = MonsterSpawnPoint::from(&test_zone_spawn_point(30));

        assert!(spawn_point.advance_spawn_check(Duration::ZERO));
        assert_eq!(spawn_point.time_since_last_check, Duration::ZERO);

        assert!(!spawn_point.advance_spawn_check(Duration::from_secs(29)));
        assert!(spawn_point.advance_spawn_check(Duration::from_secs(1)));
    }

    #[test]
    fn applying_spawn_data_preserves_runtime_alive_count_until_reset() {
        let mut spawn_point = MonsterSpawnPoint::from(&test_zone_spawn_point(30));
        spawn_point.num_alive_monsters = 3;

        let mut updated = test_zone_spawn_point(10);
        updated.limit_count = 5;
        updated.range = 25;
        spawn_point.apply_spawn_data(&updated);

        assert_eq!(spawn_point.num_alive_monsters, 3);
        assert_eq!(spawn_point.interval, Duration::from_secs(10));
        assert_eq!(spawn_point.limit_count, 5);
        assert_eq!(spawn_point.range, 25);

        spawn_point.reset_for_live_reload();

        assert_eq!(spawn_point.num_alive_monsters, 0);
        assert_eq!(spawn_point.current_tactics_value, 1);
        assert_eq!(spawn_point.time_since_last_check, Duration::from_secs(10));
    }

    #[test]
    fn live_reload_reset_can_stagger_spawn_eligibility() {
        let mut spawn_point = MonsterSpawnPoint::from(&test_zone_spawn_point(30));

        spawn_point.reset_for_live_reload_with_delay(Duration::from_secs(2));

        assert!(!spawn_point.advance_spawn_check(Duration::from_secs(1)));
        assert!(spawn_point.advance_spawn_check(Duration::from_secs(1)));
        assert_eq!(spawn_point.num_alive_monsters, 0);
        assert_eq!(spawn_point.current_tactics_value, 1);
    }
}
