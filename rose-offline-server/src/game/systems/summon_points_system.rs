use bevy::{
    ecs::prelude::{Added, Changed, Commands, Entity, Or, Query, ResMut, With, Without},
    math::{Vec3, Vec3Swizzles},
};

use crate::game::{
    bundles::client_entity_leave_zone,
    components::{
        AbilityValues, BonfireAura, ClientEntity, ClientEntitySector, Command, CommandData, Dead,
        GameClient, MoveMode, MoveSpeed, NextCommand, Owner, Position, SpawnOrigin,
        SummonPointCost, SummonUsage, BONFIRE_OWNER_LEASH_DISTANCE,
    },
    messages::server::ServerMessage,
    resources::ClientEntityList,
};

/// If a summon is farther than this from its owner, it will actively follow.
const SUMMON_FOLLOW_DISTANCE: f32 = 150.0;
/// If a summon is farther than this from its owner, teleport it instantly.
const SUMMON_TELEPORT_DISTANCE: f32 = 1500.0;
/// Speed multiplier so summons catch up to the owner.
const SUMMON_FOLLOW_SPEED_MULTIPLIER: f32 = 1.2;

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

/// Proactively makes summons follow their owner every tick.
/// - Within SUMMON_FOLLOW_DISTANCE: do nothing
/// - Beyond SUMMON_TELEPORT_DISTANCE: teleport instantly
/// - Otherwise: issue a run command toward the owner and boost speed to keep up
pub fn summon_follow_teleport_system(
    mut commands: Commands,
    query_owner: Query<(&Position, &MoveSpeed), Without<Owner>>,
    mut query_summons: Query<
        (
            Entity,
            &Owner,
            &SpawnOrigin,
            &mut Position,
            &Command,
            &AbilityValues,
        ),
        Without<Dead>,
    >,
) {
    for (
        summon_entity,
        owner,
        spawn_origin,
        mut summon_position,
        summon_command,
        summon_ability_values,
    ) in query_summons.iter_mut()
    {
        if !matches!(spawn_origin, SpawnOrigin::Summoned(_, _)) {
            continue;
        }

        let (owner_position, owner_move_speed) = match query_owner.get(owner.entity) {
            Ok(result) => result,
            Err(_) => continue,
        };

        // Only act within the same zone
        if owner_position.zone_id != summon_position.zone_id {
            continue;
        }

        let delta = owner_position.position.xy() - summon_position.position.xy();
        let distance_sq = delta.length_squared();

        // Teleport if way too far
        if distance_sq > SUMMON_TELEPORT_DISTANCE * SUMMON_TELEPORT_DISTANCE {
            let direction = if distance_sq > 0.0 {
                delta.normalize()
            } else {
                bevy::math::Vec2::X
            };
            let new_pos = owner_position.position.xy() - direction * SUMMON_FOLLOW_DISTANCE;
            summon_position.position = Vec3::new(new_pos.x, new_pos.y, 0.0);
            commands.entity(summon_entity).insert((
                Command::with_stop(),
                NextCommand::with_stop(true),
            ));
            continue;
        }

        // Close enough — don't follow
        if distance_sq <= SUMMON_FOLLOW_DISTANCE * SUMMON_FOLLOW_DISTANCE {
            continue;
        }

        // If already moving toward a destination near the owner, don't interrupt
        if let CommandData::Move { destination, .. } = summon_command.command {
            let dest_to_owner = owner_position
                .position
                .xy()
                .distance_squared(destination.xy());
            if dest_to_owner <= SUMMON_FOLLOW_DISTANCE * SUMMON_FOLLOW_DISTANCE {
                continue;
            }
        }

        // Issue a move command toward the owner and boost speed to keep up
        let direction = delta.normalize();
        let destination =
            owner_position.position.xy() - direction * (SUMMON_FOLLOW_DISTANCE * 0.5);

        let summon_run_speed = summon_ability_values.get_run_speed();
        let follow_speed =
            (owner_move_speed.speed * SUMMON_FOLLOW_SPEED_MULTIPLIER).max(summon_run_speed);

        commands.entity(summon_entity).insert((
            NextCommand::with_move(
                Vec3::new(destination.x, destination.y, 0.0),
                None,
                Some(MoveMode::Run),
            ),
            MoveSpeed::new(follow_speed),
        ));
    }
}
