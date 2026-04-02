use bevy::{
    ecs::prelude::{Added, Changed, Commands, Entity, Or, Query, ResMut, With, Without},
    math::{Vec3, Vec3Swizzles},
};

use crate::game::{
    bundles::client_entity_leave_zone,
    components::{
        AbilityValues, BonfireAura, ClientEntity, ClientEntitySector, Command, CommandData, Dead,
        GameClient, MoveMode, MoveSpeed, NextCommand, Owner, Position, SpawnOrigin,
        SummonFollowSpeedOverride, SummonPointCost, SummonUsage, BONFIRE_OWNER_LEASH_DISTANCE,
    },
    messages::server::ServerMessage,
    resources::{ClientEntityList, ServerMessages},
};

const ACTIVE_SUMMON_FOLLOW_DISTANCE: f32 = 180.0;
const ACTIVE_SUMMON_TARGET_DISTANCE: f32 = 100.0;
const ACTIVE_SUMMON_FOLLOW_SPEED_MULTIPLIER: f32 = 1.1;
const IDLE_SUMMON_FOLLOW_DISTANCE: f32 = 300.0;
const IDLE_SUMMON_TARGET_DISTANCE: f32 = 180.0;
const SUMMON_TELEPORT_DISTANCE: f32 = 1500.0;

#[derive(Clone, Copy)]
struct SummonFollowProfile {
    follow_distance: f32,
    target_distance: f32,
    follow_speed: Option<f32>,
}

fn is_idle_owner_command(command: &Command) -> bool {
    matches!(
        command.command,
        CommandData::Stop { .. }
            | CommandData::Sit
            | CommandData::Sitting
            | CommandData::Standing
            | CommandData::PersonalStore
    )
}

fn get_summon_follow_profile(
    owner_command: &Command,
    owner_move_speed: f32,
) -> SummonFollowProfile {
    if is_idle_owner_command(owner_command) {
        SummonFollowProfile {
            follow_distance: IDLE_SUMMON_FOLLOW_DISTANCE,
            target_distance: IDLE_SUMMON_TARGET_DISTANCE,
            follow_speed: None,
        }
    } else {
        SummonFollowProfile {
            follow_distance: ACTIVE_SUMMON_FOLLOW_DISTANCE,
            target_distance: ACTIVE_SUMMON_TARGET_DISTANCE,
            follow_speed: Some(owner_move_speed * ACTIVE_SUMMON_FOLLOW_SPEED_MULTIPLIER),
        }
    }
}

fn clear_summon_follow_speed_override(
    commands: &mut Commands,
    summon_entity: Entity,
    owner_entity: Entity,
    base_move_speed: f32,
    current_move_speed: f32,
    current_override: Option<&SummonFollowSpeedOverride>,
) {
    let should_restore_speed = (current_move_speed - base_move_speed).abs() > f32::EPSILON;
    if current_override.is_none() && !should_restore_speed {
        return;
    }

    commands.add(move |world: &mut bevy::ecs::world::World| {
        let Some(mut entity_mut) = world.get_entity_mut(summon_entity) else {
            log::trace!(
                target: "summon_follow",
                "Skipping clear follow speed override for missing summon entity {:?} owned by {:?}",
                summon_entity,
                owner_entity
            );
            return;
        };

        entity_mut.remove::<SummonFollowSpeedOverride>();

        if should_restore_speed {
            entity_mut.insert(MoveSpeed::new(base_move_speed));
        }
    });
}

fn set_summon_follow_speed_override(
    commands: &mut Commands,
    summon_entity: Entity,
    owner_entity: Entity,
    follow_speed: f32,
    current_move_speed: f32,
    current_override: Option<&SummonFollowSpeedOverride>,
) {
    let needs_override_update = current_override
        .map(|current_override| (current_override.speed - follow_speed).abs() > f32::EPSILON)
        .unwrap_or(true);
    let needs_move_speed_update = (current_move_speed - follow_speed).abs() > f32::EPSILON;

    if !needs_override_update && !needs_move_speed_update {
        return;
    }

    commands.add(move |world: &mut bevy::ecs::world::World| {
        let Some(mut entity_mut) = world.get_entity_mut(summon_entity) else {
            log::trace!(
                target: "summon_follow",
                "Skipping set follow speed override for missing summon entity {:?} owned by {:?}",
                summon_entity,
                owner_entity
            );
            return;
        };

        entity_mut.insert(SummonFollowSpeedOverride::new(follow_speed));

        if needs_move_speed_update {
            entity_mut.insert(MoveSpeed::new(follow_speed));
        }
    });
}

fn queue_summon_follow_teleport_reset(
    commands: &mut Commands,
    summon_entity: Entity,
    owner_entity: Entity,
    summon_base_run_speed: f32,
) {
    commands.add(move |world: &mut bevy::ecs::world::World| {
        let Some(mut entity_mut) = world.get_entity_mut(summon_entity) else {
            log::trace!(
                target: "summon_follow",
                "Skipping teleport follow reset for missing summon entity {:?} owned by {:?}",
                summon_entity,
                owner_entity
            );
            return;
        };

        entity_mut.insert((
            Command::with_stop(),
            NextCommand::with_stop(true),
            MoveSpeed::new(summon_base_run_speed),
        ));
        entity_mut.remove::<SummonFollowSpeedOverride>();
    });
}

fn queue_summon_follow_move_update(
    commands: &mut Commands,
    summon_entity: Entity,
    owner_entity: Entity,
    destination: Vec3,
    follow_speed: Option<f32>,
    summon_base_run_speed: f32,
) {
    commands.add(move |world: &mut bevy::ecs::world::World| {
        let Some(mut entity_mut) = world.get_entity_mut(summon_entity) else {
            log::trace!(
                target: "summon_follow",
                "Skipping follow move update for missing summon entity {:?} owned by {:?}",
                summon_entity,
                owner_entity
            );
            return;
        };

        entity_mut.insert(NextCommand::with_move(
            destination,
            None,
            Some(MoveMode::Run),
        ));

        if let Some(follow_speed) = follow_speed {
            entity_mut.insert((
                SummonFollowSpeedOverride::new(follow_speed),
                MoveSpeed::new(follow_speed),
            ));
        } else {
            entity_mut.remove::<SummonFollowSpeedOverride>();
            entity_mut.insert(MoveSpeed::new(summon_base_run_speed));
        }
    });
}

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
            Err(_) => true,
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

pub fn summon_follow_teleport_system(
    mut commands: Commands,
    query_owner: Query<(&Position, &MoveSpeed, &Command), Without<Owner>>,
    mut query_summons: Query<
        (
            Entity,
            &Owner,
            &SpawnOrigin,
            &mut Position,
            &Command,
            &AbilityValues,
            &MoveSpeed,
            Option<&SummonFollowSpeedOverride>,
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
        summon_move_speed,
        summon_follow_speed_override,
    ) in query_summons.iter_mut()
    {
        if !matches!(spawn_origin, SpawnOrigin::Summoned(_, _)) {
            continue;
        }

        let (owner_position, owner_move_speed, owner_command) = match query_owner.get(owner.entity)
        {
            Ok(result) => result,
            Err(_) => continue,
        };

        if owner_position.zone_id != summon_position.zone_id {
            continue;
        }

        let profile = get_summon_follow_profile(owner_command, owner_move_speed.speed);
        let summon_base_run_speed = summon_ability_values.get_run_speed();
        let delta = owner_position.position.xy() - summon_position.position.xy();
        let distance_sq = delta.length_squared();

        if distance_sq > SUMMON_TELEPORT_DISTANCE * SUMMON_TELEPORT_DISTANCE {
            let direction = if distance_sq > 0.0 {
                delta.normalize()
            } else {
                bevy::math::Vec2::X
            };
            let new_pos = owner_position.position.xy() - direction * profile.target_distance;
            summon_position.position = Vec3::new(new_pos.x, new_pos.y, 0.0);
            queue_summon_follow_teleport_reset(
                &mut commands,
                summon_entity,
                owner.entity,
                summon_base_run_speed,
            );
            continue;
        }

        if distance_sq <= profile.follow_distance * profile.follow_distance {
            clear_summon_follow_speed_override(
                &mut commands,
                summon_entity,
                owner.entity,
                summon_base_run_speed,
                summon_move_speed.speed,
                summon_follow_speed_override,
            );
            continue;
        }

        if let CommandData::Move { destination, .. } = summon_command.command {
            let dest_to_owner = owner_position
                .position
                .xy()
                .distance_squared(destination.xy());
            if dest_to_owner <= profile.follow_distance * profile.follow_distance {
                if let Some(follow_speed) = profile.follow_speed {
                    let follow_speed = follow_speed.max(summon_base_run_speed);
                    set_summon_follow_speed_override(
                        &mut commands,
                        summon_entity,
                        owner.entity,
                        follow_speed,
                        summon_move_speed.speed,
                        summon_follow_speed_override,
                    );
                } else {
                    clear_summon_follow_speed_override(
                        &mut commands,
                        summon_entity,
                        owner.entity,
                        summon_base_run_speed,
                        summon_move_speed.speed,
                        summon_follow_speed_override,
                    );
                }
                continue;
            }
        }

        let direction = delta.normalize();
        let destination = owner_position.position.xy() - direction * profile.target_distance;
        let destination = Vec3::new(destination.x, destination.y, 0.0);

        let follow_speed = profile
            .follow_speed
            .map(|follow_speed| follow_speed.max(summon_base_run_speed));
        queue_summon_follow_move_update(
            &mut commands,
            summon_entity,
            owner.entity,
            destination,
            follow_speed,
            summon_base_run_speed,
        );
    }
}

pub fn apply_summon_follow_speed_system(
    mut query: Query<
        (
            &AbilityValues,
            &MoveMode,
            &mut MoveSpeed,
            Option<&SummonFollowSpeedOverride>,
            Option<&ClientEntity>,
            Option<&SpawnOrigin>,
        ),
        Without<Dead>,
    >,
    mut server_messages: ResMut<ServerMessages>,
) {
    for (
        ability_values,
        move_mode,
        mut move_speed,
        summon_follow_speed_override,
        client_entity,
        spawn_origin,
    ) in query.iter_mut()
    {
        let is_summon = matches!(spawn_origin, Some(SpawnOrigin::Summoned(_, _)));
        if !is_summon && summon_follow_speed_override.is_none() {
            continue;
        }

        let base_move_speed = ability_values.get_move_speed(move_mode);
        let effective_move_speed = summon_follow_speed_override
            .map(|summon_follow_speed_override| summon_follow_speed_override.speed)
            .unwrap_or(base_move_speed);

        if (move_speed.speed - effective_move_speed).abs() <= f32::EPSILON {
            continue;
        }

        move_speed.speed = effective_move_speed;

        if let Some(client_entity) = client_entity {
            server_messages.send_entity_message(
                client_entity,
                ServerMessage::UpdateSpeed {
                    entity_id: client_entity.id,
                    run_speed: effective_move_speed as i32,
                    passive_attack_speed: ability_values.get_passive_attack_speed(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use bevy::{
        app::Update,
        math::Vec3,
        prelude::{App, IntoSystemConfigs},
    };
    use rose_data::ZoneId;
    use rose_game_common::components::AbilityValuesAdjust;

    use super::{
        apply_summon_follow_speed_system, summon_follow_teleport_system,
        summon_points_owner_cleanup_system,
    };
    use crate::game::{
        components::{
            AbilityValues, ClientEntity, ClientEntityId, ClientEntityType, Command, CommandData,
            DamageCategory, DamageType, MoveMode, MoveSpeed, NextCommand, Owner, Position,
            SpawnOrigin, SummonFollowSpeedOverride, SummonUsage,
        },
        messages::server::ServerMessage,
        resources::{ClientEntityList, ServerMessages},
    };

    fn test_zone() -> ZoneId {
        ZoneId::new(1).unwrap()
    }

    fn test_ability_values(run_speed: f32) -> AbilityValues {
        AbilityValues {
            is_driving: false,
            damage_category: DamageCategory::Npc,
            level: 1,
            walk_speed: 250.0,
            run_speed,
            vehicle_move_speed: 0.0,
            strength: 0,
            dexterity: 0,
            intelligence: 0,
            concentration: 0,
            charm: 0,
            sense: 0,
            max_health: 100,
            max_mana: 50,
            additional_health_recovery: 0,
            additional_mana_recovery: 0,
            attack_damage_type: DamageType::Physical,
            attack_power: 10,
            attack_speed: 100,
            passive_attack_speed: 0,
            attack_range: 150,
            hit: 1,
            defence: 1,
            resistance: 1,
            critical: 1,
            avoid: 1,
            vehicle_attack_power: 0,
            vehicle_attack_range: 0,
            vehicle_attack_speed: 0,
            vehicle_hit: 0,
            vehicle_defence: 0,
            vehicle_critical: 0,
            vehicle_avoid: 0,
            max_damage_sources: 4,
            drop_rate: 0,
            max_weight: 0,
            summon_owner_level: None,
            summon_skill_level: None,
            adjust: AbilityValuesAdjust {
                additional_damage_multiplier: 0.0,
                attack_speed: 0,
                attack_power: 0,
                avoid: 0,
                critical: 0,
                defence: 0,
                hit: 0,
                resistance: 0,
                max_health: 0,
                max_mana: 0,
                run_speed: 0.0,
            },
            npc_store_buy_rate: 0,
            npc_store_sell_rate: 0,
            save_mana: 0,
            passive_max_summons: 0,
        }
    }

    fn spawn_owner(
        app: &mut App,
        position: Vec3,
        move_speed: f32,
        command: Command,
    ) -> bevy::prelude::Entity {
        app.world
            .spawn((
                Position::new(position, test_zone()),
                MoveSpeed::new(move_speed),
                command,
            ))
            .id()
    }

    fn spawn_summon(
        app: &mut App,
        owner: bevy::prelude::Entity,
        position: Vec3,
        move_speed: f32,
        command: Command,
    ) -> bevy::prelude::Entity {
        app.world
            .spawn((
                Owner::new(owner),
                SpawnOrigin::Summoned(owner, position),
                Position::new(position, test_zone()),
                command,
                test_ability_values(400.0),
                MoveSpeed::new(move_speed),
            ))
            .id()
    }

    #[test]
    fn active_owner_beyond_follow_distance_issues_boosted_follow_move() {
        let mut app = App::new();
        app.add_systems(Update, summon_follow_teleport_system);

        let owner = spawn_owner(
            &mut app,
            Vec3::ZERO,
            500.0,
            Command::with_attack(bevy::prelude::Entity::from_raw(99), Duration::from_secs(1)),
        );
        let summon = spawn_summon(
            &mut app,
            owner,
            Vec3::new(400.0, 0.0, 0.0),
            400.0,
            Command::with_stop(),
        );

        app.update();

        let summon_ref = app.world.entity(summon);
        let next_command = summon_ref.get::<NextCommand>().unwrap();
        let move_speed = summon_ref.get::<MoveSpeed>().unwrap();
        let speed_override = summon_ref.get::<SummonFollowSpeedOverride>().unwrap();

        match next_command.command.as_ref().unwrap() {
            CommandData::Move {
                destination,
                move_mode,
                ..
            } => {
                assert_eq!(*destination, Vec3::new(100.0, 0.0, 0.0));
                assert_eq!(*move_mode, Some(MoveMode::Run));
            }
            other => panic!("expected move command, got {:?}", other),
        }

        assert_eq!(speed_override.speed, 550.0);
        assert_eq!(move_speed.speed, 550.0);
    }

    #[test]
    fn idle_owner_inside_idle_follow_distance_leaves_summon_free() {
        let mut app = App::new();
        app.add_systems(Update, summon_follow_teleport_system);

        let owner = spawn_owner(&mut app, Vec3::ZERO, 500.0, Command::with_stop());
        let summon = spawn_summon(
            &mut app,
            owner,
            Vec3::new(250.0, 0.0, 0.0),
            550.0,
            Command::with_stop(),
        );
        app.world
            .entity_mut(summon)
            .insert(SummonFollowSpeedOverride::new(550.0));

        app.update();

        let summon_ref = app.world.entity(summon);
        assert!(summon_ref.get::<NextCommand>().is_none());
        assert!(summon_ref.get::<SummonFollowSpeedOverride>().is_none());
        assert_eq!(summon_ref.get::<MoveSpeed>().unwrap().speed, 400.0);
    }

    #[test]
    fn idle_owner_beyond_idle_follow_distance_regroups_at_base_speed() {
        let mut app = App::new();
        app.add_systems(Update, summon_follow_teleport_system);

        let owner = spawn_owner(&mut app, Vec3::ZERO, 500.0, Command::with_stop());
        let summon = spawn_summon(
            &mut app,
            owner,
            Vec3::new(400.0, 0.0, 0.0),
            550.0,
            Command::with_stop(),
        );

        app.update();

        let summon_ref = app.world.entity(summon);
        let next_command = summon_ref.get::<NextCommand>().unwrap();
        assert!(summon_ref.get::<SummonFollowSpeedOverride>().is_none());
        assert_eq!(summon_ref.get::<MoveSpeed>().unwrap().speed, 400.0);

        match next_command.command.as_ref().unwrap() {
            CommandData::Move {
                destination,
                move_mode,
                ..
            } => {
                assert_eq!(*destination, Vec3::new(180.0, 0.0, 0.0));
                assert_eq!(*move_mode, Some(MoveMode::Run));
            }
            other => panic!("expected move command, got {:?}", other),
        }
    }

    #[test]
    fn teleport_clears_override_and_restores_base_speed() {
        let mut app = App::new();
        app.add_systems(Update, summon_follow_teleport_system);

        let owner = spawn_owner(&mut app, Vec3::ZERO, 500.0, Command::with_stop());
        let summon = spawn_summon(
            &mut app,
            owner,
            Vec3::new(2000.0, 0.0, 0.0),
            550.0,
            Command::with_stop(),
        );
        app.world
            .entity_mut(summon)
            .insert(SummonFollowSpeedOverride::new(550.0));

        app.update();

        let summon_ref = app.world.entity(summon);
        assert!(summon_ref.get::<SummonFollowSpeedOverride>().is_none());
        assert_eq!(summon_ref.get::<MoveSpeed>().unwrap().speed, 400.0);
        assert!(summon_ref.get::<Command>().unwrap().is_stop());

        let next_command = summon_ref.get::<NextCommand>().unwrap();
        match next_command.command.as_ref().unwrap() {
            CommandData::Stop { send_message } => assert!(*send_message),
            other => panic!("expected stop command, got {:?}", other),
        }

        assert_eq!(
            summon_ref.get::<Position>().unwrap().position,
            Vec3::new(180.0, 0.0, 0.0)
        );
    }

    #[test]
    fn apply_follow_speed_sync_only_emits_when_speed_changes() {
        let mut app = App::new();
        app.insert_resource(ServerMessages::default());
        app.add_systems(Update, apply_summon_follow_speed_system);

        let owner = app.world.spawn_empty().id();
        let summon = app
            .world
            .spawn((
                test_ability_values(400.0),
                MoveMode::Run,
                MoveSpeed::new(400.0),
                SummonFollowSpeedOverride::new(550.0),
                ClientEntity::new(ClientEntityType::Monster, ClientEntityId(10), test_zone()),
                SpawnOrigin::Summoned(owner, Vec3::ZERO),
            ))
            .id();

        app.update();

        assert_eq!(
            app.world.entity(summon).get::<MoveSpeed>().unwrap().speed,
            550.0
        );
        {
            let messages = &app
                .world
                .resource::<ServerMessages>()
                .pending_entity_messages;
            assert_eq!(messages.len(), 1);
            match &messages[0].message {
                ServerMessage::UpdateSpeed {
                    entity_id,
                    run_speed,
                    ..
                } => {
                    assert_eq!(*entity_id, ClientEntityId(10));
                    assert_eq!(*run_speed, 550);
                }
                other => panic!("expected UpdateSpeed, got {:?}", other),
            }
        }

        app.world
            .resource_mut::<ServerMessages>()
            .pending_entity_messages
            .clear();
        app.update();
        assert!(app
            .world
            .resource::<ServerMessages>()
            .pending_entity_messages
            .is_empty());

        app.world
            .entity_mut(summon)
            .remove::<SummonFollowSpeedOverride>();
        app.update();

        assert_eq!(
            app.world.entity(summon).get::<MoveSpeed>().unwrap().speed,
            400.0
        );
        let messages = &app
            .world
            .resource::<ServerMessages>()
            .pending_entity_messages;
        assert_eq!(messages.len(), 1);
        match &messages[0].message {
            ServerMessage::UpdateSpeed {
                entity_id,
                run_speed,
                ..
            } => {
                assert_eq!(*entity_id, ClientEntityId(10));
                assert_eq!(*run_speed, 400);
            }
            other => panic!("expected UpdateSpeed, got {:?}", other),
        }
    }

    #[test]
    fn owner_cleanup_despawned_summon_skips_follow_move_updates_in_same_tick() {
        let mut app = App::new();
        app.insert_resource(ClientEntityList {
            zones: HashMap::new(),
        });
        app.add_systems(
            Update,
            (
                summon_points_owner_cleanup_system,
                summon_follow_teleport_system,
            )
                .chain(),
        );

        let owner = app
            .world
            .spawn((
                Position::new(Vec3::ZERO, test_zone()),
                MoveSpeed::new(500.0),
                Command::with_attack(bevy::prelude::Entity::from_raw(99), Duration::from_secs(1)),
                SummonUsage::default(),
            ))
            .id();
        let summon = spawn_summon(
            &mut app,
            owner,
            Vec3::new(400.0, 0.0, 0.0),
            400.0,
            Command::with_stop(),
        );

        app.update();

        assert!(app.world.get_entity(summon).is_none());
    }

    #[test]
    fn owner_cleanup_despawned_summon_skips_restore_speed_updates_in_same_tick() {
        let mut app = App::new();
        app.insert_resource(ClientEntityList {
            zones: HashMap::new(),
        });
        app.add_systems(
            Update,
            (
                summon_points_owner_cleanup_system,
                summon_follow_teleport_system,
            )
                .chain(),
        );

        let owner = app
            .world
            .spawn((
                Position::new(Vec3::ZERO, test_zone()),
                MoveSpeed::new(500.0),
                Command::with_stop(),
                SummonUsage::default(),
            ))
            .id();
        let summon = spawn_summon(
            &mut app,
            owner,
            Vec3::new(250.0, 0.0, 0.0),
            550.0,
            Command::with_stop(),
        );
        app.world
            .entity_mut(summon)
            .insert(SummonFollowSpeedOverride::new(550.0));

        app.update();

        assert!(app.world.get_entity(summon).is_none());
    }
}
