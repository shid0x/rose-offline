use bevy::{
    ecs::prelude::{Commands, Entity, Query, Res, ResMut},
    time::Time,
};

use rose_data::NpcId;

use crate::game::{
    bundles::MonsterBundle,
    components::{MonsterSpawnPoint, Position, SpawnOrigin, Team},
    resources::{ClientEntityList, GameData, ZoneList},
};

fn build_spawn_queue(spawn_point: &mut MonsterSpawnPoint) -> Vec<(NpcId, usize)> {
    let live_count = spawn_point.num_alive_monsters;
    if live_count >= spawn_point.limit_count {
        spawn_point.current_tactics_value = spawn_point.current_tactics_value.saturating_sub(1);
        return Vec::new();
    }

    let regen_value =
        ((spawn_point.limit_count * 2 - live_count) * spawn_point.current_tactics_value * 50)
            / (spawn_point.limit_count * spawn_point.tactic_points);

    let mut spawn_queue: Vec<(NpcId, usize)> = Vec::new();
    match regen_value {
        0..=10 => {
            // Spawn basic[0]
            spawn_point.current_tactics_value += 12;
            if let Some((id, count)) = spawn_point.basic_spawns.get(0) {
                spawn_queue.push((*id, *count))
            }
        }
        11..=15 => {
            // Spawn basic[0] - 2, basic[1]
            spawn_point.current_tactics_value += 15;
            if let Some((id, count)) = spawn_point.basic_spawns.get(0) {
                spawn_queue.push((*id, count.saturating_sub(2)))
            }
            if let Some((id, count)) = spawn_point.basic_spawns.get(1) {
                spawn_queue.push((*id, *count))
            }
        }
        16..=25 => {
            // Spawn basic[2]
            spawn_point.current_tactics_value += 12;
            if let Some((id, count)) = spawn_point.basic_spawns.get(2) {
                spawn_queue.push((*id, *count))
            }
        }
        26..=30 => {
            // Spawn basic[0] - 1, basic[2]
            spawn_point.current_tactics_value += 15;
            if let Some((id, count)) = spawn_point.basic_spawns.get(0) {
                spawn_queue.push((*id, count.saturating_sub(1)))
            }
            if let Some((id, count)) = spawn_point.basic_spawns.get(2) {
                spawn_queue.push((*id, *count))
            }
        }
        31..=40 => {
            // Spawn basic[3]
            spawn_point.current_tactics_value += 12;
            if let Some((id, count)) = spawn_point.basic_spawns.get(3) {
                spawn_queue.push((*id, *count))
            }
        }
        41..=50 => {
            // Spawn basic[1], basic[2] - 2
            spawn_point.current_tactics_value += 12;
            if let Some((id, count)) = spawn_point.basic_spawns.get(1) {
                spawn_queue.push((*id, *count))
            }
            if let Some((id, count)) = spawn_point.basic_spawns.get(2) {
                spawn_queue.push((*id, count.saturating_sub(1)))
            }
        }
        51..=65 => {
            // Spawn basic[2], basic[3] - 2
            spawn_point.current_tactics_value += 20;
            if let Some((id, count)) = spawn_point.basic_spawns.get(2) {
                spawn_queue.push((*id, *count))
            }
            if let Some((id, count)) = spawn_point.basic_spawns.get(3) {
                spawn_queue.push((*id, count.saturating_sub(2)))
            }
        }
        66..=73 => {
            // Spawn basic[3], basic[4]
            spawn_point.current_tactics_value += 15;
            if let Some((id, count)) = spawn_point.basic_spawns.get(3) {
                spawn_queue.push((*id, *count))
            }
            if let Some((id, count)) = spawn_point.basic_spawns.get(4) {
                spawn_queue.push((*id, *count))
            }
        }
        74..=85 => {
            // Spawn basic[0], basic[4] - 2, tactics[0] - 1
            spawn_point.current_tactics_value += 15;
            if let Some((id, count)) = spawn_point.basic_spawns.get(0) {
                spawn_queue.push((*id, *count))
            }
            if let Some((id, count)) = spawn_point.basic_spawns.get(4) {
                spawn_queue.push((*id, count.saturating_sub(2)))
            }
            if let Some((id, count)) = spawn_point.tactic_spawns.get(0) {
                spawn_queue.push((*id, count.saturating_sub(1)))
            }
        }
        86..=92 => {
            // Spawn basic[1], tactics[0], tactics[1]
            spawn_point.current_tactics_value = 1;
            if let Some((id, count)) = spawn_point.basic_spawns.get(1) {
                spawn_queue.push((*id, *count))
            }
            if let Some((id, count)) = spawn_point.tactic_spawns.get(0) {
                spawn_queue.push((*id, *count))
            }
            if let Some((id, count)) = spawn_point.tactic_spawns.get(1) {
                spawn_queue.push((*id, *count))
            }
        }
        _ => {
            // Spawn basic[4], tactics[0] + 1, tactics[1]
            spawn_point.current_tactics_value = 7;
            if let Some((id, count)) = spawn_point.basic_spawns.get(4) {
                spawn_queue.push((*id, *count))
            }
            if let Some((id, count)) = spawn_point.tactic_spawns.get(0) {
                spawn_queue.push((*id, count + 1))
            }
            if let Some((id, count)) = spawn_point.tactic_spawns.get(1) {
                spawn_queue.push((*id, *count))
            }
        }
    }

    if spawn_point.current_tactics_value > 500 {
        spawn_point.current_tactics_value = 500;
    }

    spawn_queue
}

pub fn monster_spawn_system(
    mut commands: Commands,
    mut query: Query<(Entity, &mut MonsterSpawnPoint, &Position)>,
    time: Res<Time>,
    mut client_entity_list: ResMut<ClientEntityList>,
    game_data: Res<GameData>,
    zone_list: Res<ZoneList>,
) {
    query.for_each_mut(
        |(spawn_point_entity, mut spawn_point, spawn_point_position)| {
            if !zone_list.get_monster_spawns_enabled(spawn_point_position.zone_id) {
                return;
            }

            let spawn_point = &mut *spawn_point;
            if !spawn_point.advance_spawn_check(time.delta()) {
                return;
            }
            let spawn_queue = build_spawn_queue(spawn_point);

            let spawn_point_zone = spawn_point_position.zone_id;
            let spawn_point_position = spawn_point_position.position;
            let spawn_range = (spawn_point.range * 100) as i32;

            for (npc_id, count) in spawn_queue {
                for _ in 0..count {
                    if MonsterBundle::spawn(
                        &mut commands,
                        &mut client_entity_list,
                        &game_data,
                        npc_id,
                        spawn_point_zone,
                        SpawnOrigin::MonsterSpawnPoint(spawn_point_entity, spawn_point_position),
                        spawn_range,
                        Team::default_monster(),
                        None,
                        None,
                    )
                    .is_some()
                    {
                        spawn_point.num_alive_monsters += 1;
                    }
                }
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::math::Vec3;
    use rose_data::{NpcId, ZoneId};

    use super::build_spawn_queue;
    use crate::game::components::{MonsterSpawnPoint, Position};

    fn test_spawn_point() -> MonsterSpawnPoint {
        MonsterSpawnPoint {
            basic_spawns: vec![(NpcId::new(1).unwrap(), 1)],
            tactic_spawns: Vec::new(),
            interval: Duration::from_secs(30),
            limit_count: 1,
            range: 0,
            tactic_points: 100,
            time_since_last_check: Duration::from_secs(30),
            current_tactics_value: 1,
            num_alive_monsters: 0,
        }
    }

    #[test]
    fn first_spawn_cycle_produces_a_monster_without_waiting() {
        let mut spawn_point = test_spawn_point();
        let _position = Position::new(Vec3::ZERO, ZoneId::new(11).unwrap());

        assert!(spawn_point.advance_spawn_check(Duration::ZERO));

        let spawn_queue = build_spawn_queue(&mut spawn_point);

        assert_eq!(spawn_queue, vec![(NpcId::new(1).unwrap(), 1)]);
        assert_eq!(spawn_point.current_tactics_value, 13);
    }

    #[test]
    fn spawn_queue_does_not_over_spawn_past_limit_count() {
        let mut spawn_point = test_spawn_point();
        spawn_point.current_tactics_value = 10;
        spawn_point.num_alive_monsters = 1;

        let spawn_queue = build_spawn_queue(&mut spawn_point);

        assert!(spawn_queue.is_empty());
        assert_eq!(spawn_point.current_tactics_value, 9);
    }
}
