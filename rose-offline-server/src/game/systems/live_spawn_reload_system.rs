use std::time::Duration;

use bevy::ecs::{
    prelude::{Commands, Entity, Query, ResMut},
    query::Without,
    system::ParamSet,
};

use crate::game::{
    bundles::client_entity_leave_zone,
    components::{
        ClientEntity, ClientEntitySector, GameClient, MonsterSpawnPoint, MonsterSpawnSource,
        Position, SpawnOrigin,
    },
    resources::{
        ClientEntityList, LiveSpawnReloadJob, LiveSpawnReloadQueue,
        LIVE_SPAWN_RELOAD_DESPAWNS_PER_TICK, LIVE_SPAWN_RELOAD_STAGGER,
    },
};

pub fn live_spawn_reload_system(
    mut commands: Commands,
    mut live_spawn_reload_queue: ResMut<LiveSpawnReloadQueue>,
    mut client_entity_list: ResMut<ClientEntityList>,
    mut queries: ParamSet<(
        Query<
            (
                Entity,
                &MonsterSpawnSource,
                &mut MonsterSpawnPoint,
                &mut Position,
            ),
            Without<GameClient>,
        >,
        Query<
            (
                &SpawnOrigin,
                &ClientEntity,
                &ClientEntitySector,
                &Position,
            ),
            Without<GameClient>,
        >,
    )>,
) {
    let mut ready_to_apply = false;

    if let Some(job) = live_spawn_reload_queue.front_mut() {
        let spawned_monster_query = queries.p1();
        let mut despawned_this_tick = 0;
        while despawned_this_tick < LIVE_SPAWN_RELOAD_DESPAWNS_PER_TICK {
            let Some(monster_entity) = job.pending_monsters.pop_front() else {
                break;
            };
            let Ok((spawn_origin, client_entity, client_entity_sector, position)) =
                spawned_monster_query.get(monster_entity)
            else {
                continue;
            };
            let SpawnOrigin::MonsterSpawnPoint(spawn_point_entity, _) = spawn_origin else {
                continue;
            };
            if !job.old_spawn_entities.contains(spawn_point_entity) {
                continue;
            }

            client_entity_leave_zone(
                &mut commands,
                &mut client_entity_list,
                monster_entity,
                client_entity,
                client_entity_sector,
                position,
            );
            commands.entity(monster_entity).despawn();
            despawned_this_tick += 1;
        }

        ready_to_apply = job.pending_monsters.is_empty();
    }

    if ready_to_apply {
        let Some(job) = live_spawn_reload_queue.pop_front() else {
            return;
        };
        apply_live_spawn_reload_job(&mut commands, &mut queries.p0(), job);
    }
}

fn apply_live_spawn_reload_job(
    commands: &mut Commands,
    spawn_point_query: &mut Query<
        (
            Entity,
            &MonsterSpawnSource,
            &mut MonsterSpawnPoint,
            &mut Position,
        ),
        Without<GameClient>,
    >,
    job: LiveSpawnReloadJob,
) {
    let mut found_spawns = vec![false; job.reloaded_spawns.len()];

    for (entity, source, mut spawn_point, mut position) in spawn_point_query.iter_mut() {
        if source.zone_id != job.zone_id
            || source.block_x != job.block_x
            || source.block_y != job.block_y
        {
            continue;
        }

        if let Some(reloaded_spawn) = job.reloaded_spawns.get(source.spawn_index) {
            found_spawns[source.spawn_index] = true;
            spawn_point.apply_spawn_data(reloaded_spawn);
            spawn_point.reset_for_live_reload_with_delay(live_reload_spawn_delay(
                source.spawn_index,
                job.reloaded_spawns.len(),
            ));
            position.position = reloaded_spawn.position;
        } else {
            commands.entity(entity).despawn();
        }
    }

    for (source_spawn_index, reloaded_spawn) in job.reloaded_spawns.iter().enumerate() {
        if found_spawns[source_spawn_index] {
            continue;
        }

        let mut spawn_point = MonsterSpawnPoint::from(reloaded_spawn);
        spawn_point.reset_for_live_reload_with_delay(live_reload_spawn_delay(
            source_spawn_index,
            job.reloaded_spawns.len(),
        ));
        commands.spawn((
            spawn_point,
            MonsterSpawnSource::new(job.zone_id, job.block_x, job.block_y, source_spawn_index),
            Position::new(reloaded_spawn.position, job.zone_id),
        ));
    }
}

fn live_reload_spawn_delay(spawn_index: usize, spawn_count: usize) -> Duration {
    if spawn_count <= 1 {
        return Duration::ZERO;
    }

    LIVE_SPAWN_RELOAD_STAGGER.mul_f32(spawn_index as f32 / spawn_count as f32)
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, time::Duration};

    use bevy::{ecs::entity::Entity, utils::HashSet};
    use rose_data::ZoneId;

    use crate::game::resources::{LiveSpawnReloadJob, LiveSpawnReloadQueue};

    use super::live_reload_spawn_delay;

    #[test]
    fn live_reload_queue_blocks_old_spawn_entities() {
        let spawn_entity = Entity::from_raw(42);
        let mut old_spawn_entities = HashSet::new();
        old_spawn_entities.insert(spawn_entity);
        let mut queue = LiveSpawnReloadQueue::default();

        queue.push(LiveSpawnReloadJob::new(
            ZoneId::new(1).unwrap(),
            2,
            3,
            Vec::new(),
            old_spawn_entities,
            VecDeque::new(),
        ));

        assert!(queue.is_spawn_point_blocked(spawn_entity));
        assert!(!queue.is_spawn_point_blocked(Entity::from_raw(43)));
    }

    #[test]
    fn live_reload_spawn_delay_staggers_across_window() {
        assert_eq!(live_reload_spawn_delay(0, 4), Duration::ZERO);
        assert_eq!(live_reload_spawn_delay(2, 4), Duration::from_secs(1));
        assert!(live_reload_spawn_delay(3, 4) < Duration::from_secs(2));
    }
}
