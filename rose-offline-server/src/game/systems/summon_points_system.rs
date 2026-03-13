use bevy::{
    ecs::prelude::{Added, Changed, Commands, Entity, Or, Query, ResMut, With},
    math::Vec3Swizzles,
};

use crate::game::{
    bundles::client_entity_leave_zone,
    components::{
        AbilityValues, BonfireAura, ClientEntity, ClientEntitySector, Command, Dead, GameClient,
        Owner, Position, SpawnOrigin, SummonPointCost, SummonUsage, BONFIRE_OWNER_LEASH_DISTANCE,
    },
    messages::server::ServerMessage,
    resources::ClientEntityList,
};

fn send_summon_points_update(
    game_client: &GameClient,
    ability_values: &AbilityValues,
    summon_usage: &SummonUsage,
) {
    game_client
        .server_message_tx
        .send(ServerMessage::UpdateSummonPoints {
            used_points: summon_usage.used_points.min(u16::MAX as u32) as u16,
            max_points: ability_values.get_max_summon_points().min(u16::MAX as u32) as u16,
        })
        .ok();
}

pub fn summon_points_sync_system(
    query: Query<
        (&GameClient, &AbilityValues, &SummonUsage),
        (
            With<ClientEntity>,
            Or<(
                Added<ClientEntity>,
                Changed<AbilityValues>,
                Changed<SummonUsage>,
            )>,
        ),
    >,
) {
    for (game_client, ability_values, summon_usage) in query.iter() {
        send_summon_points_update(game_client, ability_values, summon_usage);
    }
}

pub fn summon_points_dead_cleanup_system(
    mut commands: Commands,
    mut query_owner_usage: Query<&mut SummonUsage>,
    query_dead_summons: Query<
        (Entity, &Owner, &SpawnOrigin, &SummonPointCost, &Command),
        (With<Dead>, With<ClientEntity>),
    >,
) {
    for (summon_entity, owner, spawn_origin, summon_point_cost, command) in
        query_dead_summons.iter()
    {
        if !matches!(spawn_origin, SpawnOrigin::Summoned(_, _)) {
            continue;
        }

        let command_complete = command
            .required_duration
            .map_or(false, |required_duration| {
                command.duration >= required_duration
            });
        if !command_complete {
            continue;
        }

        if let Ok(mut summon_usage) = query_owner_usage.get_mut(owner.entity) {
            summon_usage.used_points = summon_usage
                .used_points
                .saturating_sub(summon_point_cost.points);
        }

        // Mark this summon as already accounted for so we don't decrement twice.
        commands.entity(summon_entity).remove::<SummonPointCost>();
    }
}

pub fn summon_points_owner_cleanup_system(
    mut commands: Commands,
    mut client_entity_list: ResMut<ClientEntityList>,
    mut query_owner_state: Query<(
        Option<&Dead>,
        Option<&ClientEntity>,
        &Position,
        Option<&mut SummonUsage>,
    )>,
    query_summons: Query<(
        Entity,
        &Owner,
        &SpawnOrigin,
        Option<&ClientEntity>,
        Option<&ClientEntitySector>,
        Option<&BonfireAura>,
        &Position,
        Option<&SummonPointCost>,
    )>,
) {
    for (
        summon_entity,
        owner,
        spawn_origin,
        summon_client_entity,
        summon_client_entity_sector,
        bonfire_aura,
        summon_position,
        summon_point_cost,
    ) in query_summons.iter()
    {
        if !matches!(spawn_origin, SpawnOrigin::Summoned(_, _)) {
            continue;
        }

        let should_cleanup = match query_owner_state.get_mut(owner.entity) {
            Ok((owner_dead, owner_client_entity, owner_position, owner_summon_usage)) => {
                let should_cleanup = owner_dead.is_some()
                    || owner_client_entity.is_none()
                    || owner_position.zone_id != summon_position.zone_id
                    || bonfire_aura.is_some()
                        && owner_position
                            .position
                            .xy()
                            .distance_squared(summon_position.position.xy())
                            > BONFIRE_OWNER_LEASH_DISTANCE * BONFIRE_OWNER_LEASH_DISTANCE;

                if should_cleanup {
                    if let (Some(mut owner_summon_usage), Some(summon_point_cost)) =
                        (owner_summon_usage, summon_point_cost)
                    {
                        owner_summon_usage.used_points = owner_summon_usage
                            .used_points
                            .saturating_sub(summon_point_cost.points);
                    }
                }

                should_cleanup
            }
            Err(_) => {
                // Owner no longer exists (for example logout/despawn); summon must be cleaned up.
                true
            }
        };

        if !should_cleanup {
            continue;
        }

        if let (Some(client_entity), Some(client_entity_sector)) =
            (summon_client_entity, summon_client_entity_sector)
        {
            client_entity_leave_zone(
                &mut commands,
                &mut client_entity_list,
                summon_entity,
                client_entity,
                client_entity_sector,
                summon_position,
            );
        }

        commands.entity(summon_entity).despawn();
    }
}

pub fn summon_points_owner_reset_system(
    mut query_owner_state: Query<(Option<&Dead>, Option<&ClientEntity>, &mut SummonUsage)>,
) {
    for (owner_dead, owner_client_entity, mut summon_usage) in query_owner_state.iter_mut() {
        if owner_dead.is_some() || owner_client_entity.is_none() {
            summon_usage.used_points = 0;
        }
    }
}
