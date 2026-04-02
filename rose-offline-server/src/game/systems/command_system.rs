use std::time::{Duration, Instant};

use bevy::{
    ecs::{
        prelude::{Commands, Entity, EventWriter, Query, Res, ResMut},
        query::WorldQuery,
    },
    math::{Vec3, Vec3Swizzles},
    time::Time,
};

use rose_data::{
    AmmoIndex, EquipmentIndex, ItemClass, SkillActionMode, SkillData, SkillId, SkillType,
    VehiclePartIndex,
};
use rose_game_common::components::{CharacterGender, CharacterInfo};

use crate::game::{
    bundles::{
        skill_can_target_entity, skill_can_target_position, skill_can_target_self, skill_can_use,
        SkillCasterBundle, SkillCasterBundleItem, SkillTargetBundle,
    },
    components::{
        AbilityValues, ClientEntity, ClientEntitySector, ClientEntityType, Command,
        CommandCastSkillTarget, CommandData, Equipment, GameClient, HealthPoints, ItemDrop,
        MotionData, MoveMode, MoveSpeed, NextCommand, Npc, Owner, PartyOwner, PersonalStore,
        Position, StatusEffects, Team,
    },
    events::{
        DamageEvent, ItemLifeEvent, PickupItemEvent, SkillEvent, SkillEventTarget, UseAmmoEvent,
    },
    messages::server::{CancelCastingSkillReason, ServerMessage},
    pvp::can_character_attack_character,
    resources::{GameData, ServerMessages, ZoneList},
};

const NPC_MOVE_TO_DISTANCE: f32 = 250.0;
const CHARACTER_MOVE_TO_DISTANCE: f32 = 1000.0;
const DROPPED_ITEM_MOVE_TO_DISTANCE: f32 = 150.0;
const DROPPED_ITEM_PICKUP_DISTANCE: f32 = 200.0;

#[derive(WorldQuery)]
#[world_query(mutable)]
pub struct QueryCommandEntity<'w> {
    entity: Entity,

    command: &'w mut Command,
    next_command: &'w mut NextCommand,

    ability_values: &'w AbilityValues,
    client_entity: &'w ClientEntity,
    motion_data: &'w MotionData,
    move_mode: &'w MoveMode,
    position: &'w Position,
    status_effects: &'w StatusEffects,
    team: &'w Team,

    character_info: Option<&'w CharacterInfo>,
    equipment: Option<&'w Equipment>,
    game_client: Option<&'w GameClient>,
    npc: Option<&'w Npc>,
    personal_store: Option<&'w PersonalStore>,
}

#[derive(WorldQuery)]
pub struct CommandAttackTargetQuery<'w> {
    ability_values: &'w AbilityValues,
    client_entity: &'w ClientEntity,
    health_points: &'w HealthPoints,
    position: &'w Position,
    team: &'w Team,
}

#[derive(WorldQuery)]
pub struct CommandMoveTargetQuery<'w> {
    client_entity: &'w ClientEntity,
    position: &'w Position,
}

#[derive(WorldQuery)]
#[world_query(mutable)]
pub struct CommandPickupItemTargetQuery<'w> {
    client_entity: &'w ClientEntity,
    client_entity_sector: &'w ClientEntitySector,
    item_drop: &'w mut ItemDrop,
    position: &'w Position,
    owner: Option<&'w Owner>,
    party_owner: Option<&'w PartyOwner>,
}

fn command_stop(
    command: &mut Command,
    client_entity: &ClientEntity,
    position: &Position,
    server_messages: Option<&mut ServerMessages>,
) {
    if let Some(server_messages) = server_messages {
        server_messages.send_entity_message(
            client_entity,
            ServerMessage::StopMoveEntity {
                entity_id: client_entity.id,
                x: position.position.x,
                y: position.position.y,
                z: position.position.z as u16,
            },
        );
    }

    *command = Command::with_stop();
}

fn command_is_blocked_by_action_disable(command: &CommandData) -> bool {
    !matches!(command, CommandData::Stop { .. } | CommandData::Die { .. })
}

fn command_is_blocked_by_skill_disable(command: &CommandData) -> bool {
    matches!(command, CommandData::CastSkill { .. })
}

fn clear_skill_use_disabled_target(
    command: &mut Command,
    next_command: &mut NextCommand,
    client_entity: &ClientEntity,
    position: &Position,
    server_messages: &mut ServerMessages,
) -> bool {
    let should_stop_current = command_is_blocked_by_skill_disable(&command.command);
    let should_clear_next = next_command
        .command
        .as_ref()
        .is_some_and(command_is_blocked_by_skill_disable);

    if should_stop_current {
        command_stop(command, client_entity, position, Some(server_messages));
    }

    if should_clear_next {
        *next_command = NextCommand::default();
    }

    should_stop_current
}

fn get_skill_followup_command(
    action_mode: SkillActionMode,
    target_entity: Option<Entity>,
    current_command: &CommandData,
    stored_restore_command: Option<&CommandData>,
) -> NextCommand {
    match action_mode {
        SkillActionMode::Stop => NextCommand::default(),
        SkillActionMode::Attack => target_entity.map_or_else(NextCommand::default, |target| {
            NextCommand::with_command_skip_server_message(CommandData::Attack { target })
        }),
        SkillActionMode::Restore => {
            if let Some(restore_command) = stored_restore_command {
                return NextCommand::with_command_skip_server_message(restore_command.clone());
            }

            match current_command {
                CommandData::Stop { .. }
                | CommandData::Move { .. }
                | CommandData::Attack { .. } => {
                    NextCommand::with_command_skip_server_message(current_command.clone())
                }
                CommandData::Die { .. }
                | CommandData::Emote { .. }
                | CommandData::PickupItemDrop { .. }
                | CommandData::PersonalStore
                | CommandData::Sit
                | CommandData::Sitting
                | CommandData::Standing
                | CommandData::CastSkill { .. } => NextCommand::default(),
            }
        }
    }
}

fn is_valid_move_target(target: &CommandMoveTargetQueryItem, position: &Position) -> bool {
    if target.position.zone_id != position.zone_id {
        return false;
    }

    true
}

fn is_valid_attack_target(
    target: &CommandAttackTargetQueryItem,
    client_entity: &ClientEntity,
    position: &Position,
    team: &Team,
    game_data: &GameData,
    zone_list: &ZoneList,
) -> bool {
    if target.client_entity.id == client_entity.id {
        return false;
    }

    if target.position.zone_id != position.zone_id {
        return false;
    }

    if target.health_points.hp <= 0 {
        return false;
    }

    if matches!(client_entity.entity_type, ClientEntityType::Character)
        && matches!(
            target.client_entity.entity_type,
            ClientEntityType::Character
        )
    {
        let Some(zone_data) = game_data.zones.get_zone(position.zone_id) else {
            return false;
        };

        return can_character_attack_character(
            zone_data,
            zone_list.get_pvp_enabled(position.zone_id),
            team.id,
            target.team.id,
        );
    }

    if target.team.id == team.id || target.team.id == Team::DEFAULT_NPC_TEAM_ID {
        return false;
    }

    true
}

fn is_valid_pickup_target(target: &CommandPickupItemTargetQueryItem, position: &Position) -> bool {
    if target.position.zone_id != position.zone_id {
        return false;
    }

    let distance = position
        .position
        .xy()
        .distance(target.position.position.xy());
    if distance > DROPPED_ITEM_PICKUP_DISTANCE {
        return false;
    }

    true
}

fn can_cast_skill(
    now: Instant,
    game_data: &GameData,
    zone_list: &ZoneList,
    command_entity: Entity,
    target: &Option<CommandCastSkillTarget>,
    skill_id: SkillId,
    query_skill_caster: &Query<SkillCasterBundle>,
    query_skill_target: &Query<SkillTargetBundle>,
) -> bool {
    let Ok(skill_caster) = query_skill_caster.get(command_entity) else {
        return false;
    };

    let Some(skill_data) = game_data.skills.get_skill(skill_id) else {
        return false;
    };

    if !skill_can_use(now, game_data, &skill_caster, skill_data) {
        return false;
    }

    match target {
        Some(CommandCastSkillTarget::Entity(target_entity)) => {
            let Ok(skill_target) = query_skill_target.get(*target_entity) else {
                return false;
            };

            if !skill_can_target_entity(
                game_data,
                zone_list,
                &skill_caster,
                &skill_target,
                skill_data,
            ) {
                return false;
            }
        }
        Some(CommandCastSkillTarget::Position(_)) => {
            if !skill_can_target_position(skill_data) {
                return false;
            }
        }
        None => {
            if !matches!(
                skill_data.skill_type,
                SkillType::SelfBoundDuration
                    | SkillType::SelfBound
                    | SkillType::SelfStateDuration
                    | SkillType::SummonPet
                    | SkillType::SelfDamage
            ) && !skill_can_target_self(game_data, zone_list, &skill_caster, skill_data)
            {
                return false;
            }
        }
    }

    true
}

fn is_summon_points_limited(
    game_data: &GameData,
    skill_caster: &SkillCasterBundleItem,
    skill_data: &SkillData,
) -> bool {
    if !matches!(skill_data.skill_type, SkillType::SummonPet) {
        return false;
    }

    let Some(summon_npc_id) = skill_data.summon_npc_id else {
        return false;
    };
    let Some(summon_npc_data) = game_data.npcs.get_npc(summon_npc_id) else {
        return false;
    };

    if summon_npc_data.summon_point_requirement == 0 {
        return false;
    }

    let used_points = skill_caster
        .summon_usage
        .map_or(0, |summon_usage| summon_usage.used_points);
    used_points.saturating_add(summon_npc_data.summon_point_requirement)
        > skill_caster.ability_values.get_max_summon_points()
}

fn sync_npc_combat_chase_move_speed(
    commands: &mut Commands,
    entity: Entity,
    ability_values: &AbilityValues,
    npc: Option<&Npc>,
) {
    if npc.is_none() {
        return;
    }

    let move_mode = MoveMode::Run;
    commands.entity(entity).insert((
        move_mode,
        MoveSpeed::new(ability_values.get_move_speed(&move_mode)),
    ));
}

pub fn command_system(
    mut commands: Commands,
    mut query_command_entity: Query<QueryCommandEntity>,
    query_move_target: Query<CommandMoveTargetQuery>,
    query_attack_target: Query<CommandAttackTargetQuery>,
    mut query_pickup_item: Query<CommandPickupItemTargetQuery>,
    query_skill_target: Query<SkillTargetBundle>,
    query_skill_caster: Query<SkillCasterBundle>,
    game_data: Res<GameData>,
    zone_list: Res<ZoneList>,
    time: Res<Time>,
    mut damage_events: EventWriter<DamageEvent>,
    mut skill_events: EventWriter<SkillEvent>,
    mut pickup_item_event: EventWriter<PickupItemEvent>,
    mut item_life_event: EventWriter<ItemLifeEvent>,
    mut use_ammo_event: EventWriter<UseAmmoEvent>,
    mut server_messages: ResMut<ServerMessages>,
) {
    let Some(now) = time.last_update() else {
        return;
    };

    for mut command_entity in query_command_entity.iter_mut() {
        if command_entity.command.is_dead() {
            // Ignore all requested commands whilst dead.
            command_entity.next_command.command = None;
        }

        if !command_entity.command.is_dead() && command_entity.status_effects.is_action_disabled() {
            let should_stop_current =
                command_is_blocked_by_action_disable(&command_entity.command.command);
            let should_clear_next = command_entity.next_command.command.is_some();

            if should_stop_current {
                command_stop(
                    &mut command_entity.command,
                    command_entity.client_entity,
                    command_entity.position,
                    Some(&mut server_messages),
                );
            }

            if should_clear_next {
                *command_entity.next_command = NextCommand::default();
            }

            continue;
        }

        if !command_entity.command.is_dead()
            && command_entity.status_effects.is_skill_use_disabled()
            && clear_skill_use_disabled_target(
                &mut command_entity.command,
                &mut command_entity.next_command,
                command_entity.client_entity,
                command_entity.position,
                &mut server_messages,
            )
        {
            continue;
        }

        if !command_entity.next_command.has_sent_server_message
            && command_entity.next_command.command.is_some()
        {
            // Send any server message required for update client next command
            match command_entity.next_command.command.as_mut().unwrap() {
                CommandData::Die { .. } => {
                    panic!("Next command should never be set to die, set current command")
                }
                CommandData::Sit | CommandData::Sitting | CommandData::Standing => {}
                CommandData::Stop { .. } => {}
                CommandData::PersonalStore => {}
                CommandData::PickupItemDrop { .. } => {}
                CommandData::Emote { .. } => {}
                CommandData::Move {
                    destination,
                    target,
                    move_mode: command_move_mode,
                } => {
                    let mut target_entity_id = None;
                    if let Some(target_entity) = *target {
                        if let Some(target) = query_move_target
                            .get(target_entity)
                            .ok()
                            .filter(|target| is_valid_move_target(target, command_entity.position))
                        {
                            *destination = target.position.position;
                            target_entity_id = Some(target.client_entity.id);
                        } else {
                            *target = None;
                        }
                    }

                    let distance = command_entity
                        .position
                        .position
                        .xy()
                        .distance(destination.xy());
                    server_messages.send_entity_message(
                        command_entity.client_entity,
                        ServerMessage::MoveEntity {
                            entity_id: command_entity.client_entity.id,
                            target_entity_id,
                            distance: distance as u16,
                            x: destination.x,
                            y: destination.y,
                            z: destination.z as u16,
                            move_mode: *command_move_mode,
                        },
                    );
                }
                &mut CommandData::Attack {
                    target: target_entity,
                } => {
                    if let Some(target) =
                        query_attack_target
                            .get(target_entity)
                            .ok()
                            .filter(|target| {
                                is_valid_attack_target(
                                    target,
                                    command_entity.client_entity,
                                    command_entity.position,
                                    command_entity.team,
                                    &game_data,
                                    &zone_list,
                                )
                            })
                    {
                        let distance = command_entity
                            .position
                            .position
                            .xy()
                            .distance(target.position.position.xy());

                        server_messages.send_entity_message(
                            command_entity.client_entity,
                            ServerMessage::AttackEntity {
                                entity_id: command_entity.client_entity.id,
                                target_entity_id: target.client_entity.id,
                                distance: distance as u16,
                                x: target.position.position.x,
                                y: target.position.position.y,
                                z: target.position.position.z as u16,
                            },
                        );
                    } else {
                        *command_entity.next_command = NextCommand::with_stop(true);
                    }
                }
                &mut CommandData::CastSkill {
                    skill_id,
                    ref skill_target,
                    cast_motion_id,
                    ..
                } => {
                    if can_cast_skill(
                        now,
                        &game_data,
                        &zone_list,
                        command_entity.entity,
                        skill_target,
                        skill_id,
                        &query_skill_caster,
                        &query_skill_target,
                    ) {
                        match skill_target {
                            Some(CommandCastSkillTarget::Entity(target_entity)) => {
                                let skill_target = query_skill_target.get(*target_entity).unwrap();
                                let distance = command_entity
                                    .position
                                    .position
                                    .xy()
                                    .distance(skill_target.position.position.xy());

                                server_messages.send_entity_message(
                                    command_entity.client_entity,
                                    ServerMessage::CastSkillTargetEntity {
                                        entity_id: command_entity.client_entity.id,
                                        skill_id,
                                        target_entity_id: skill_target.client_entity.id,
                                        target_distance: distance,
                                        target_position: skill_target.position.position.xy(),
                                        cast_motion_id,
                                    },
                                );
                            }
                            Some(CommandCastSkillTarget::Position(target_position)) => {
                                server_messages.send_entity_message(
                                    command_entity.client_entity,
                                    ServerMessage::CastSkillTargetPosition {
                                        entity_id: command_entity.client_entity.id,
                                        skill_id,
                                        target_position: *target_position,
                                        cast_motion_id,
                                    },
                                );
                            }
                            None => {
                                server_messages.send_entity_message(
                                    command_entity.client_entity,
                                    ServerMessage::CastSkillSelf {
                                        entity_id: command_entity.client_entity.id,
                                        skill_id,
                                        cast_motion_id,
                                    },
                                );
                            }
                        }
                    }
                }
            }

            command_entity.next_command.has_sent_server_message = true;
        }

        command_entity.command.duration += time.delta();

        let required_duration = match &mut command_entity.command.command {
            CommandData::Attack { .. } => {
                let attack_speed =
                    i32::max(command_entity.ability_values.get_attack_speed(), 30) as f32 / 100.0;
                command_entity
                    .command
                    .required_duration
                    .map(|duration| duration.div_f32(attack_speed))
            }
            CommandData::Emote { .. } => {
                // Any command can interrupt an emote
                if command_entity.next_command.command.is_some() {
                    None
                } else {
                    command_entity.command.required_duration
                }
            }
            _ => command_entity.command.required_duration,
        };

        let command_motion_completed = required_duration.map_or_else(
            || true,
            |required_duration| command_entity.command.duration >= required_duration,
        );

        if !command_motion_completed {
            // Current command still in animation
            continue;
        }

        match command_entity.command.command {
            CommandData::Die { .. } => {
                // We can't perform NextCommand if we are dead!
                continue;
            }
            CommandData::Sitting => {
                // When sitting animation is complete transition to Sit
                *command_entity.command = Command::with_sit();
            }
            _ => {}
        }

        if command_entity.next_command.command.is_none() {
            // If we have completed current command, and there is no next command, then clear current.
            // This does not apply for some commands which must be manually completed, such as Sit
            // where you need to stand after.
            if command_motion_completed && !command_entity.command.command.is_manual_complete() {
                *command_entity.command = Command::default();
            }

            // Nothing to do when there is no next command
            continue;
        }

        if matches!(command_entity.command.command, CommandData::Sit) {
            // If current command is sit, we must stand before performing NextCommand
            let duration = command_entity
                .motion_data
                .get_sit_standing()
                .map(|motion_data| motion_data.duration)
                .unwrap_or_else(|| Duration::from_secs(0));

            *command_entity.command = Command::with_standing(duration);

            server_messages.send_entity_message(
                command_entity.client_entity,
                ServerMessage::SitToggle {
                    entity_id: command_entity.client_entity.id,
                },
            );
            continue;
        }

        let weapon_item_data = command_entity.equipment.as_ref().and_then(|equipment| {
            equipment
                .get_equipment_item(EquipmentIndex::Weapon)
                .and_then(|weapon_item| {
                    game_data
                        .items
                        .get_weapon_item(weapon_item.item.item_number)
                })
        });
        let weapon_motion_type = weapon_item_data
            .map(|weapon_item_data| weapon_item_data.motion_type as usize)
            .unwrap_or(0);
        let weapon_motion_gender = command_entity
            .character_info
            .map(|character_info| match character_info.gender {
                CharacterGender::Male => 0,
                CharacterGender::Female => 1,
            })
            .unwrap_or(0);

        match command_entity.next_command.command.as_mut().unwrap() {
            &mut CommandData::Stop { send_message } => {
                command_stop(
                    &mut command_entity.command,
                    command_entity.client_entity,
                    command_entity.position,
                    if send_message {
                        Some(&mut server_messages)
                    } else {
                        None
                    },
                );
                *command_entity.next_command = NextCommand::default();
            }
            CommandData::Move {
                destination,
                target,
                move_mode: command_move_mode,
            } => {
                let mut entity_commands = commands.entity(command_entity.entity);

                if let Some(target_entity) = *target {
                    if let Some(target) = query_move_target
                        .get(target_entity)
                        .ok()
                        .filter(|target| is_valid_move_target(target, command_entity.position))
                    {
                        let required_distance = match target.client_entity.entity_type {
                            ClientEntityType::Character => Some(CHARACTER_MOVE_TO_DISTANCE),
                            ClientEntityType::Npc => Some(NPC_MOVE_TO_DISTANCE),
                            ClientEntityType::ItemDrop => Some(DROPPED_ITEM_MOVE_TO_DISTANCE),
                            _ => None,
                        };

                        if let Some(required_distance) = required_distance {
                            let distance = command_entity
                                .position
                                .position
                                .xy()
                                .distance(target.position.position.xy());
                            if distance < required_distance {
                                // We are already within required distance, so no need to move further
                                *destination = command_entity.position.position;
                            } else {
                                let offset = (target.position.position.xy()
                                    - command_entity.position.position.xy())
                                .normalize()
                                    * required_distance;
                                destination.x = target.position.position.x - offset.x;
                                destination.y = target.position.position.y - offset.y;
                                destination.z = target.position.position.z;
                            }
                        } else {
                            *destination = target.position.position;
                        }
                    } else {
                        *target = None;
                    }
                }

                // If this move command has a different move mode, update move mode and move speed
                if let Some(command_move_mode) = command_move_mode.as_ref() {
                    if command_move_mode != command_entity.move_mode {
                        entity_commands.insert((
                            *command_move_mode,
                            MoveSpeed::new(
                                command_entity
                                    .ability_values
                                    .get_move_speed(command_move_mode),
                            ),
                        ));
                    }
                }

                let distance = command_entity
                    .position
                    .position
                    .xy()
                    .distance(destination.xy());
                if distance < 0.1 {
                    *command_entity.command = Command::with_stop();
                } else {
                    *command_entity.command =
                        Command::with_move(*destination, *target, *command_move_mode);
                }
            }
            &mut CommandData::PickupItemDrop {
                target: target_entity,
            } => {
                if query_pickup_item
                    .get_mut(target_entity)
                    .ok()
                    .map_or(false, |target| {
                        is_valid_pickup_target(&target, command_entity.position)
                    })
                {
                    pickup_item_event.send(PickupItemEvent {
                        pickup_entity: command_entity.entity,
                        item_entity: target_entity,
                    });

                    // Update our current command
                    let motion_duration = command_entity
                        .motion_data
                        .get_pickup_item_drop()
                        .map_or_else(|| Duration::from_secs(1), |motion| motion.duration);

                    *command_entity.command =
                        Command::with_pickup_item_drop(target_entity, motion_duration);
                } else {
                    *command_entity.command = Command::with_stop();
                }

                *command_entity.next_command = NextCommand::default();
            }
            &mut CommandData::Attack {
                target: target_entity,
            } => {
                let Some(target) = query_attack_target
                    .get(target_entity)
                    .ok()
                    .filter(|target| {
                        is_valid_attack_target(
                            target,
                            command_entity.client_entity,
                            command_entity.position,
                            command_entity.team,
                            &game_data,
                            &zone_list,
                        )
                    })
                else {
                    // Cannot attack target, cancel command.
                    command_stop(
                        &mut command_entity.command,
                        command_entity.client_entity,
                        command_entity.position,
                        Some(&mut server_messages),
                    );
                    *command_entity.next_command = NextCommand::default();
                    continue;
                };

                let attack_range = command_entity.ability_values.get_attack_range() as f32;
                let distance = command_entity
                    .position
                    .position
                    .xy()
                    .distance(target.position.position.xy());
                if attack_range < distance {
                    sync_npc_combat_chase_move_speed(
                        &mut commands,
                        command_entity.entity,
                        command_entity.ability_values,
                        command_entity.npc,
                    );

                    // Not in range, set current command to move
                    *command_entity.command = Command::with_move(
                        target.position.position,
                        Some(target_entity),
                        Some(MoveMode::Run),
                    );
                    continue;
                }

                let mut cancel_attack = false;

                let (attack_duration, hit_count) =
                    if let Some(attack_motion) = command_entity.motion_data.get_attack() {
                        (attack_motion.duration, attack_motion.total_attack_frames)
                    } else {
                        // No attack animation, cancel attack
                        cancel_attack = true;
                        (Duration::ZERO, 0)
                    };

                if matches!(command_entity.move_mode, MoveMode::Drive) {
                    if let Some(equipment) = command_entity.equipment.as_ref() {
                        if equipment
                            .get_vehicle_item(VehiclePartIndex::Engine)
                            .map_or(false, |equipment_item| equipment_item.life == 0)
                        {
                            // Vehicle engine is broken, cancel attack
                            cancel_attack = true;
                        }

                        if equipment
                            .get_vehicle_item(VehiclePartIndex::Arms)
                            .map_or(false, |equipment_item| equipment_item.life == 0)
                        {
                            // Vehicle weapon item is broken, cancel attack
                            cancel_attack = true;
                        }
                    }
                } else {
                    if let Some(equipment) = command_entity.equipment.as_ref() {
                        if equipment
                            .get_equipment_item(EquipmentIndex::Weapon)
                            .map_or(false, |equipment_item| equipment_item.life == 0)
                        {
                            // Weapon item is broken, cancel attack
                            cancel_attack = true;
                        }
                    }

                    // If the weapon uses ammo, we must consume the ammo
                    if !cancel_attack {
                        if let Some(equipment) = command_entity.equipment {
                            if let Some(weapon_item_data) = weapon_item_data {
                                let ammo_index = match weapon_item_data.item_data.class {
                                    ItemClass::Bow | ItemClass::Crossbow => Some(AmmoIndex::Arrow),
                                    ItemClass::Gun | ItemClass::DualGuns => Some(AmmoIndex::Bullet),
                                    ItemClass::Launcher => Some(AmmoIndex::Throw),
                                    _ => None,
                                };

                                if let Some(ammo_index) = ammo_index {
                                    if equipment
                                        .get_ammo_item(ammo_index)
                                        .map_or(false, |ammo_item| {
                                            ammo_item.quantity >= hit_count as u32
                                        })
                                    {
                                        use_ammo_event.send(UseAmmoEvent {
                                            entity: command_entity.entity,
                                            ammo_index,
                                            quantity: hit_count,
                                        });
                                    } else {
                                        // Not enough ammo, cancel attack
                                        cancel_attack = true;
                                    }
                                }
                            }
                        }
                    }
                }

                if cancel_attack {
                    // Attack requirements not met, cancel attack
                    command_stop(
                        &mut command_entity.command,
                        command_entity.client_entity,
                        command_entity.position,
                        Some(&mut server_messages),
                    );
                    *command_entity.next_command = NextCommand::default();
                    continue;
                }

                if matches!(command_entity.move_mode, MoveMode::Drive) {
                    // Decrease vehicle engine item life on attack
                    item_life_event.send(ItemLifeEvent::DecreaseVehicleEngineLife {
                        entity: command_entity.entity,
                        amount: None,
                    });
                }

                // Decrease weapon item life on attack
                if command_entity.character_info.is_some() {
                    item_life_event.send(ItemLifeEvent::DecreaseWeaponLife {
                        entity: command_entity.entity,
                    });
                }

                // In range, set current command to attack
                *command_entity.command = Command::with_attack(target_entity, attack_duration);

                // Send damage event to damage system
                damage_events.send(DamageEvent::Attack {
                    attacker: command_entity.entity,
                    defender: target_entity,
                    damage: game_data.ability_value_calculator.calculate_damage(
                        command_entity.ability_values,
                        target.ability_values,
                        hit_count as i32,
                    ),
                });
            }
            &mut CommandData::CastSkill {
                skill_id,
                skill_target,
                ref use_item,
                cast_motion_id,
                action_motion_id,
                ref restore_command,
            } => {
                let stored_restore_command = restore_command.as_deref().cloned();

                if !can_cast_skill(
                    now,
                    &game_data,
                    &zone_list,
                    command_entity.entity,
                    &skill_target,
                    skill_id,
                    &query_skill_caster,
                    &query_skill_target,
                ) {
                    if let (Ok(skill_caster), Some(skill_data)) = (
                        query_skill_caster.get(command_entity.entity),
                        game_data.skills.get_skill(skill_id),
                    ) {
                        if is_summon_points_limited(&game_data, &skill_caster, skill_data) {
                            server_messages.send_entity_message(
                                command_entity.client_entity,
                                ServerMessage::CancelCastingSkill {
                                    entity_id: command_entity.client_entity.id,
                                    reason: CancelCastingSkillReason::NeedSummonPoints,
                                },
                            );
                        }
                    }

                    // Cannot use skill (e.g. insufficient MP). Discard the cast request but
                    // preserve current combat intent to avoid breaking auto-attack state.
                    if let Some(restore_command) = stored_restore_command.as_ref() {
                        *command_entity.next_command =
                            NextCommand::with_command_skip_server_message(restore_command.clone());
                    } else if let Some(target_entity) = command_entity.command.target_entity() {
                        *command_entity.next_command =
                            NextCommand::with_command_skip_server_message(CommandData::Attack {
                                target: target_entity,
                            });
                    } else {
                        *command_entity.next_command = NextCommand::default();
                    }
                    continue;
                }

                let skill_data = game_data.skills.get_skill(skill_id).unwrap();

                let (target_position, target_entity) = match skill_target {
                    Some(CommandCastSkillTarget::Entity(target_entity)) => {
                        let skill_target = query_skill_target.get(target_entity).unwrap();
                        (Some(skill_target.position.position), Some(target_entity))
                    }
                    Some(CommandCastSkillTarget::Position(target_position)) => (
                        Some(Vec3::new(target_position.x, target_position.y, 0.0)),
                        None,
                    ),
                    None => (None, None),
                };

                let cast_range = if skill_data.cast_range > 0 {
                    skill_data.cast_range as f32
                } else {
                    command_entity.ability_values.get_attack_range() as f32
                };

                let in_distance = target_position.map_or(true, |target_position| {
                    command_entity
                        .position
                        .position
                        .xy()
                        .distance_squared(target_position.xy())
                        < cast_range * cast_range
                });
                if !in_distance {
                    sync_npc_combat_chase_move_speed(
                        &mut commands,
                        command_entity.entity,
                        command_entity.ability_values,
                        command_entity.npc,
                    );

                    // Temporary movement to reach cast range should not overwrite the
                    // pre-skill combat intent stored on the queued skill command.
                    *command_entity.command = Command::with_move(
                        target_position.unwrap(),
                        target_entity,
                        Some(MoveMode::Run),
                    );
                    continue;
                }

                let casting_duration = cast_motion_id
                    .or(skill_data.casting_motion_id)
                    .and_then(|motion_id| {
                        if let Some(npc) = command_entity.npc {
                            game_data.npcs.get_npc_motion(npc.id, motion_id)
                        } else {
                            game_data.motions.find_first_character_motion(
                                motion_id,
                                weapon_motion_type,
                                weapon_motion_gender,
                            )
                        }
                    })
                    .map(|motion_data| motion_data.duration)
                    .unwrap_or_else(|| Duration::from_secs(0))
                    .mul_f32(skill_data.casting_motion_speed);

                let action_duration = action_motion_id
                    .or(skill_data.action_motion_id)
                    .and_then(|motion_id| {
                        if let Some(npc) = command_entity.npc {
                            game_data.npcs.get_npc_motion(npc.id, motion_id)
                        } else {
                            game_data.motions.find_first_character_motion(
                                motion_id,
                                weapon_motion_type,
                                weapon_motion_gender,
                            )
                        }
                    })
                    .map(|motion_data| motion_data.duration)
                    .unwrap_or_else(|| Duration::from_secs(0))
                    .mul_f32(skill_data.action_motion_speed);

                // For skills which target an entity, we must send a message indicating start of skill
                if target_entity.is_some() {
                    server_messages.send_entity_message(
                        command_entity.client_entity,
                        ServerMessage::StartCastingSkill {
                            entity_id: command_entity.client_entity.id,
                        },
                    );
                }

                // Send skill event for effect to be applied after casting motion
                skill_events.send(SkillEvent::new(
                    command_entity.entity,
                    time.last_update().unwrap() + casting_duration,
                    skill_id,
                    match skill_target {
                        None => SkillEventTarget::Entity(command_entity.entity),
                        Some(CommandCastSkillTarget::Entity(target_entity)) => {
                            SkillEventTarget::Entity(target_entity)
                        }
                        Some(CommandCastSkillTarget::Position(target_position)) => {
                            SkillEventTarget::Position(target_position)
                        }
                    },
                    use_item.clone(),
                ));

                // Update next command
                *command_entity.next_command = get_skill_followup_command(
                    skill_data.action_mode,
                    target_entity,
                    &command_entity.command.command,
                    stored_restore_command.as_ref(),
                );

                // Set current command to cast skill
                *command_entity.command = Command::with_cast_skill_restore(
                    skill_id,
                    skill_target,
                    casting_duration,
                    action_duration,
                    stored_restore_command,
                );
            }
            CommandData::PersonalStore => {
                let personal_store = command_entity.personal_store.unwrap();
                server_messages.send_entity_message(
                    command_entity.client_entity,
                    ServerMessage::OpenPersonalStore {
                        entity_id: command_entity.client_entity.id,
                        skin: personal_store.skin,
                        title: personal_store.title.clone(),
                    },
                );

                *command_entity.command = Command::with_personal_store();
                *command_entity.next_command = NextCommand::default();
            }
            CommandData::Sitting => {
                let duration = command_entity
                    .motion_data
                    .get_sit_sitting()
                    .map(|motion_data| motion_data.duration)
                    .unwrap_or_else(|| Duration::from_secs(0));

                *command_entity.command = Command::with_sitting(duration);
                *command_entity.next_command = NextCommand::default();

                server_messages.send_entity_message(
                    command_entity.client_entity,
                    ServerMessage::SitToggle {
                        entity_id: command_entity.client_entity.id,
                    },
                );
            }
            CommandData::Standing => {
                // The transition from Sit to Standing happens above
                *command_entity.next_command = NextCommand::default();
            }
            CommandData::Sit => {
                // The transition from Sitting to Sit happens above
                *command_entity.next_command = NextCommand::default();
            }
            &mut CommandData::Emote { motion_id, is_stop } => {
                let motion_data = if let Some(npc) = command_entity.npc {
                    game_data.npcs.get_npc_motion(npc.id, motion_id)
                } else {
                    game_data.motions.find_first_character_motion(
                        motion_id,
                        weapon_motion_type,
                        weapon_motion_gender,
                    )
                };

                // We wait to send emote message until now as client applies it immediately
                server_messages.send_entity_message(
                    command_entity.client_entity,
                    ServerMessage::UseEmote {
                        entity_id: command_entity.client_entity.id,
                        motion_id,
                        is_stop,
                    },
                );

                let duration = motion_data
                    .map(|motion_data| motion_data.duration)
                    .unwrap_or_else(|| Duration::from_secs(0));

                *command_entity.command = Command::with_emote(motion_id, is_stop, duration);
                *command_entity.next_command = NextCommand::default();
            }
            CommandData::Die { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::Arc,
        time::{Duration, Instant},
    };

    use bevy::{
        app::{App, Update},
        math::{UVec2, Vec3},
        time::Time,
    };
    use rose_data::{CharacterMotionDatabaseOptions, NpcDatabaseOptions, SkillActionMode, ZoneId};
    use rose_data_irose::{
        get_ai_database, get_character_motion_database, get_data_decoder, get_item_database,
        get_job_class_database, get_npc_database, get_product_database, get_quest_database,
        get_skill_database, get_status_effect_database, get_string_database,
        get_warp_gate_database, get_zone_database,
    };
    use rose_file_readers::{HostFilesystemDevice, VirtualFilesystem};
    use rose_game_common::components::{BasicStats, CharacterGender};
    use rose_game_irose::data::{get_ability_value_calculator, get_drop_table};

    use super::{
        clear_skill_use_disabled_target, command_is_blocked_by_action_disable,
        command_is_blocked_by_skill_disable, command_system, get_skill_followup_command,
    };
    use crate::game::{
        components::{
            ClientEntity, ClientEntityId, ClientEntityType, Command, CommandData, NextCommand,
            Position,
        },
        components::{
            ClientEntitySector, Cooldowns, HealthPoints, MotionData, MoveMode, MoveSpeed, Npc,
            StatusEffects, Team,
        },
        events::{DamageEvent, ItemLifeEvent, PickupItemEvent, SkillEvent, UseAmmoEvent},
        resources::{GameData, ServerMessages, ZoneList},
        storage::character::{CharacterCreator, CharacterCreatorError, CharacterStorage},
    };

    #[test]
    fn action_disabled_commands_allow_only_stop_and_die() {
        assert!(!command_is_blocked_by_action_disable(&CommandData::Stop {
            send_message: false,
        }));
        assert!(!command_is_blocked_by_action_disable(&CommandData::Die {
            killer: None,
            damage: None,
        }));
        assert!(command_is_blocked_by_action_disable(&CommandData::Move {
            destination: Default::default(),
            target: None,
            move_mode: None,
        }));
        assert!(command_is_blocked_by_action_disable(&CommandData::Attack {
            target: bevy::prelude::Entity::from_raw(1),
        }));
        assert!(command_is_blocked_by_action_disable(&CommandData::Emote {
            motion_id: rose_data::MotionId::new(1),
            is_stop: false,
        }));
        assert!(command_is_blocked_by_action_disable(&CommandData::Sitting));
        assert!(command_is_blocked_by_action_disable(&CommandData::Standing));
        assert!(command_is_blocked_by_action_disable(&CommandData::Sit));
        assert!(command_is_blocked_by_action_disable(
            &CommandData::PersonalStore
        ));
        assert!(command_is_blocked_by_action_disable(
            &CommandData::PickupItemDrop {
                target: bevy::prelude::Entity::from_raw(2),
            }
        ));
        assert!(command_is_blocked_by_action_disable(
            &CommandData::CastSkill {
                skill_id: rose_data::SkillId::new(1).unwrap(),
                skill_target: None,
                use_item: None,
                cast_motion_id: None,
                action_motion_id: None,
                restore_command: None,
            }
        ));
    }

    #[test]
    fn skill_disabled_only_blocks_cast_skill_commands() {
        assert!(!command_is_blocked_by_skill_disable(&CommandData::Stop {
            send_message: false,
        }));
        assert!(!command_is_blocked_by_skill_disable(&CommandData::Move {
            destination: Default::default(),
            target: None,
            move_mode: None,
        }));
        assert!(!command_is_blocked_by_skill_disable(&CommandData::Attack {
            target: bevy::prelude::Entity::from_raw(1),
        }));
        assert!(command_is_blocked_by_skill_disable(
            &CommandData::CastSkill {
                skill_id: rose_data::SkillId::new(1).unwrap(),
                skill_target: None,
                use_item: None,
                cast_motion_id: None,
                action_motion_id: None,
                restore_command: None,
            }
        ));
    }

    #[test]
    fn restore_skill_followup_prefers_stored_attack_command() {
        let target_entity = bevy::prelude::Entity::from_raw(7);
        let current_command = CommandData::Move {
            destination: Vec3::new(10.0, 0.0, 0.0),
            target: Some(target_entity),
            move_mode: Some(MoveMode::Run),
        };
        let stored_restore = CommandData::Attack {
            target: target_entity,
        };

        let next_command = get_skill_followup_command(
            SkillActionMode::Restore,
            Some(target_entity),
            &current_command,
            Some(&stored_restore),
        );

        assert!(matches!(
            next_command.command,
            Some(CommandData::Attack { target }) if target == target_entity
        ));
        assert!(next_command.has_sent_server_message);
    }

    #[test]
    fn restore_skill_followup_uses_current_command_when_no_stored_restore_exists() {
        let target_entity = bevy::prelude::Entity::from_raw(8);
        let current_command = CommandData::Move {
            destination: Vec3::new(20.0, 0.0, 0.0),
            target: Some(target_entity),
            move_mode: Some(MoveMode::Run),
        };

        let next_command = get_skill_followup_command(
            SkillActionMode::Restore,
            Some(target_entity),
            &current_command,
            None,
        );

        assert!(matches!(
            next_command.command,
            Some(CommandData::Move {
                target: Some(target),
                ..
            }) if target == target_entity
        ));
        assert!(next_command.has_sent_server_message);
    }

    #[test]
    fn attack_and_stop_skill_followups_keep_existing_behavior() {
        let target_entity = bevy::prelude::Entity::from_raw(9);
        let current_command = CommandData::Stop {
            send_message: false,
        };

        let attack_followup = get_skill_followup_command(
            SkillActionMode::Attack,
            Some(target_entity),
            &current_command,
            None,
        );
        let stop_followup = get_skill_followup_command(
            SkillActionMode::Stop,
            Some(target_entity),
            &current_command,
            None,
        );

        assert!(matches!(
            attack_followup.command,
            Some(CommandData::Attack { target }) if target == target_entity
        ));
        assert!(attack_followup.has_sent_server_message);
        assert!(stop_followup.command.is_none());
    }

    #[test]
    fn skill_disabled_clears_only_cast_commands() {
        let mut command = Command::with_move(Vec3::new(10.0, 20.0, 0.0), None, Some(MoveMode::Run));
        let mut next_command =
            NextCommand::with_cast_skill_target_self(rose_data::SkillId::new(1).unwrap(), None);
        let client_entity = ClientEntity::new(
            ClientEntityType::Character,
            ClientEntityId(1),
            ZoneId::new(1).unwrap(),
        );
        let position = Position::new(Vec3::ZERO, ZoneId::new(1).unwrap());
        let mut server_messages = ServerMessages::default();

        let stopped_current = clear_skill_use_disabled_target(
            &mut command,
            &mut next_command,
            &client_entity,
            &position,
            &mut server_messages,
        );

        assert!(!stopped_current);
        assert!(matches!(command.command, CommandData::Move { .. }));
        assert!(next_command.command.is_none());
        assert!(server_messages.pending_entity_messages.is_empty());
    }

    #[test]
    fn skill_disabled_stops_active_cast_and_notifies_client() {
        let mut command = Command::with_cast_skill(
            rose_data::SkillId::new(1).unwrap(),
            None,
            Default::default(),
            Default::default(),
        );
        let mut next_command = NextCommand::default();
        let client_entity = ClientEntity::new(
            ClientEntityType::Character,
            ClientEntityId(2),
            ZoneId::new(1).unwrap(),
        );
        let position = Position::new(Vec3::new(5.0, 6.0, 7.0), ZoneId::new(1).unwrap());
        let mut server_messages = ServerMessages::default();

        let stopped_current = clear_skill_use_disabled_target(
            &mut command,
            &mut next_command,
            &client_entity,
            &position,
            &mut server_messages,
        );

        assert!(stopped_current);
        assert!(command.is_stop());
        assert_eq!(server_messages.pending_entity_messages.len(), 1);
    }

    #[test]
    fn npc_attack_chase_switches_to_run_move_mode_and_speed() {
        let game_data = load_test_game_data();
        let npc_data = game_data
            .npcs
            .iter()
            .find(|npc| npc.attack_range > 0)
            .expect("expected at least one attack-capable npc in test data");
        let npc_id = npc_data.id;
        let zone_id = game_data
            .zones
            .iter()
            .next()
            .expect("expected at least one zone in test data")
            .id;
        let ability_values = game_data
            .ability_value_calculator
            .calculate_npc(npc_id, &StatusEffects::default(), None, None)
            .expect("expected npc ability values");
        let walk_speed = ability_values.get_move_speed(&MoveMode::Walk);
        let run_speed = ability_values.get_move_speed(&MoveMode::Run);
        let far_target_position = Vec3::new(
            ability_values.get_attack_range().max(100) as f32 * 2.0 + 500.0,
            0.0,
            0.0,
        );

        let mut app = App::new();
        app.insert_resource(Time::default());
        app.insert_resource(ServerMessages::default());
        app.insert_resource(ZoneList::new());
        app.insert_resource(game_data);
        app.add_event::<DamageEvent>();
        app.add_event::<SkillEvent>();
        app.add_event::<PickupItemEvent>();
        app.add_event::<ItemLifeEvent>();
        app.add_event::<UseAmmoEvent>();
        app.add_systems(Update, command_system);

        let target_entity = app
            .world
            .spawn((
                ClientEntity::new(ClientEntityType::Character, ClientEntityId(2), zone_id),
                ability_values.clone(),
                HealthPoints::new(100),
                Position::new(far_target_position, zone_id),
                Team::default_character(),
            ))
            .id();

        let caster_entity = app
            .world
            .spawn((
                ClientEntity::new(ClientEntityType::Monster, ClientEntityId(1), zone_id),
                ClientEntitySector::new(UVec2::ZERO),
                Command::with_stop(),
                NextCommand::with_attack(target_entity),
                ability_values,
                MotionData::from_npc(&app.world.resource::<GameData>().npcs, npc_id),
                MoveMode::Walk,
                MoveSpeed::new(walk_speed),
                Position::new(Vec3::ZERO, zone_id),
                StatusEffects::default(),
                Team::default_monster(),
                HealthPoints::new(100),
                Npc::new(npc_id, 0),
                Cooldowns::default(),
            ))
            .id();

        advance_time(&mut app, Duration::from_millis(50));
        app.update();

        let command = app
            .world
            .get::<Command>(caster_entity)
            .expect("expected current command after chase starts");
        let move_mode = app
            .world
            .get::<MoveMode>(caster_entity)
            .expect("expected move mode after chase starts");
        let move_speed = app
            .world
            .get::<MoveSpeed>(caster_entity)
            .expect("expected move speed after chase starts");

        assert!(matches!(
            command.command,
            CommandData::Move {
                target: Some(target),
                move_mode: Some(MoveMode::Run),
                ..
            } if target == target_entity
        ));
        assert_eq!(*move_mode, MoveMode::Run);
        assert_eq!(move_speed.speed, run_speed);
    }

    #[test]
    fn npc_cast_skill_chase_switches_to_run_move_mode_and_speed() {
        let game_data = load_test_game_data();
        let npc_data = game_data
            .npcs
            .iter()
            .find(|npc| npc.attack_range > 0)
            .expect("expected at least one attack-capable npc in test data");
        let npc_id = npc_data.id;
        let zone_id = game_data
            .zones
            .iter()
            .next()
            .expect("expected at least one zone in test data")
            .id;
        let ability_values = game_data
            .ability_value_calculator
            .calculate_npc(npc_id, &StatusEffects::default(), None, None)
            .expect("expected npc ability values");
        let walk_speed = ability_values.get_move_speed(&MoveMode::Walk);
        let run_speed = ability_values.get_move_speed(&MoveMode::Run);
        let far_target_position = Vec3::new(
            ability_values.get_attack_range().max(100) as f32 * 2.0 + 500.0,
            0.0,
            0.0,
        );
        let skill_id = rose_data::SkillId::new(2983).expect("expected restore-mode npc skill");
        let cast_motion_id = rose_data::MotionId::new(8);
        let action_motion_id = rose_data::MotionId::new(9);

        let mut app = App::new();
        app.insert_resource(Time::default());
        app.insert_resource(ServerMessages::default());
        app.insert_resource(ZoneList::new());
        app.insert_resource(game_data);
        app.add_event::<DamageEvent>();
        app.add_event::<SkillEvent>();
        app.add_event::<PickupItemEvent>();
        app.add_event::<ItemLifeEvent>();
        app.add_event::<UseAmmoEvent>();
        app.add_systems(Update, command_system);

        let target_entity = app
            .world
            .spawn((
                ClientEntity::new(ClientEntityType::Character, ClientEntityId(2), zone_id),
                ability_values.clone(),
                HealthPoints::new(100),
                Position::new(far_target_position, zone_id),
                Team::default_character(),
            ))
            .id();

        let caster_entity = app
            .world
            .spawn((
                ClientEntity::new(ClientEntityType::Monster, ClientEntityId(1), zone_id),
                ClientEntitySector::new(UVec2::ZERO),
                Command::with_stop(),
                NextCommand::with_npc_cast_skill_target(
                    skill_id,
                    target_entity,
                    cast_motion_id,
                    action_motion_id,
                    Some(CommandData::Attack {
                        target: target_entity,
                    }),
                ),
                ability_values,
                MotionData::from_npc(&app.world.resource::<GameData>().npcs, npc_id),
                MoveMode::Walk,
                MoveSpeed::new(walk_speed),
                Position::new(Vec3::ZERO, zone_id),
                StatusEffects::default(),
                Team::default_monster(),
                HealthPoints::new(100),
                Npc::new(npc_id, 0),
                Cooldowns::default(),
            ))
            .id();

        advance_time(&mut app, Duration::from_millis(50));
        app.update();

        let command = app
            .world
            .get::<Command>(caster_entity)
            .expect("expected current command after cast chase starts");
        let move_mode = app
            .world
            .get::<MoveMode>(caster_entity)
            .expect("expected move mode after cast chase starts");
        let move_speed = app
            .world
            .get::<MoveSpeed>(caster_entity)
            .expect("expected move speed after cast chase starts");

        assert!(matches!(
            command.command,
            CommandData::Move {
                target: Some(target),
                move_mode: Some(MoveMode::Run),
                ..
            } if target == target_entity
        ));
        assert_eq!(*move_mode, MoveMode::Run);
        assert_eq!(move_speed.speed, run_speed);
    }

    #[test]
    fn npc_restore_skill_from_chase_resumes_attack_after_cast() {
        let game_data = load_test_game_data();
        let npc_data = game_data
            .npcs
            .iter()
            .find(|npc| npc.attack_range > 0)
            .expect("expected at least one attack-capable npc in test data");
        let npc_id = npc_data.id;
        let zone_id = game_data
            .zones
            .iter()
            .next()
            .expect("expected at least one zone in test data")
            .id;
        let ability_values = game_data
            .ability_value_calculator
            .calculate_npc(npc_id, &StatusEffects::default(), None, None)
            .expect("expected npc ability values");
        let target_ability_values = ability_values.clone();
        let attack_range = ability_values.get_attack_range().max(100) as f32;
        let far_target_position = Vec3::new(attack_range * 2.0 + 500.0, 0.0, 0.0);
        let near_target_position = Vec3::new(attack_range / 2.0, 0.0, 0.0);
        let skill_id = rose_data::SkillId::new(2983).expect("expected restore-mode npc skill");
        let cast_motion_id = rose_data::MotionId::new(8);
        let action_motion_id = rose_data::MotionId::new(9);

        let mut app = App::new();
        app.insert_resource(Time::default());
        app.insert_resource(ServerMessages::default());
        app.insert_resource(ZoneList::new());
        app.insert_resource(game_data);
        app.add_event::<DamageEvent>();
        app.add_event::<SkillEvent>();
        app.add_event::<PickupItemEvent>();
        app.add_event::<ItemLifeEvent>();
        app.add_event::<UseAmmoEvent>();
        app.add_systems(Update, command_system);

        let target_entity = app
            .world
            .spawn((
                ClientEntity::new(ClientEntityType::Character, ClientEntityId(2), zone_id),
                target_ability_values,
                HealthPoints::new(100),
                Position::new(far_target_position, zone_id),
                Team::default_character(),
            ))
            .id();

        let caster_entity = app
            .world
            .spawn((
                ClientEntity::new(ClientEntityType::Monster, ClientEntityId(1), zone_id),
                ClientEntitySector::new(UVec2::ZERO),
                Command::with_move(
                    far_target_position,
                    Some(target_entity),
                    Some(MoveMode::Run),
                ),
                NextCommand::with_npc_cast_skill_target(
                    skill_id,
                    target_entity,
                    cast_motion_id,
                    action_motion_id,
                    Some(CommandData::Attack {
                        target: target_entity,
                    }),
                ),
                ability_values,
                MotionData::from_npc(&app.world.resource::<GameData>().npcs, npc_id),
                MoveMode::Run,
                Position::new(Vec3::ZERO, zone_id),
                StatusEffects::default(),
                Team::default_monster(),
                HealthPoints::new(100),
                Npc::new(npc_id, 0),
                Cooldowns::default(),
            ))
            .id();

        advance_time(&mut app, Duration::from_millis(50));
        app.update();

        app.world
            .entity_mut(target_entity)
            .insert(Position::new(near_target_position, zone_id));
        app.world
            .entity_mut(caster_entity)
            .insert(Position::new(Vec3::ZERO, zone_id));

        advance_time(&mut app, Duration::from_millis(50));
        app.update();

        let next_command = app
            .world
            .get::<NextCommand>(caster_entity)
            .expect("expected caster next command after cast starts");
        let current_command = app
            .world
            .get::<Command>(caster_entity)
            .expect("expected current command after cast starts");
        assert!(
            matches!(
                next_command.command,
                Some(CommandData::Attack { target }) if target == target_entity
            ),
            "expected attack followup, got next={:?} current={:?}",
            next_command.command,
            current_command.command
        );

        {
            let mut entity = app.world.entity_mut(caster_entity);
            let mut command = entity
                .get_mut::<Command>()
                .expect("expected cast skill command after second update");
            command.duration = command.required_duration.unwrap_or(Duration::ZERO);
        }

        advance_time(&mut app, Duration::from_millis(50));
        app.update();

        let command = app
            .world
            .get::<Command>(caster_entity)
            .expect("expected command after cast completes");
        let next_command = app
            .world
            .get::<NextCommand>(caster_entity)
            .expect("expected next command after cast completes");
        assert!(
            matches!(
                command.command,
                CommandData::Attack { target } if target == target_entity
            ),
            "expected current attack command, got current={:?} next={:?}",
            command.command,
            next_command.command
        );
    }

    fn load_test_game_data() -> GameData {
        let assets_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let vfs = VirtualFilesystem::new(vec![Box::new(HostFilesystemDevice::new(assets_root))]);
        let string_database = get_string_database(&vfs, 1).expect("failed to load string database");
        let item_database = Arc::new(
            get_item_database(&vfs, string_database.clone()).expect("failed to load item database"),
        );
        let npc_database = Arc::new(
            get_npc_database(
                &vfs,
                string_database.clone(),
                &NpcDatabaseOptions {
                    load_frame_data: true,
                },
            )
            .expect("failed to load npc database"),
        );
        let job_class_database = Arc::new(
            get_job_class_database(&vfs, string_database.clone())
                .expect("failed to load job class database"),
        );
        let skill_database = Arc::new(
            get_skill_database(&vfs, string_database.clone())
                .expect("failed to load skill database"),
        );
        let zone_database = Arc::new(
            get_zone_database(&vfs, string_database.clone()).expect("failed to load zone database"),
        );
        let drop_table = get_drop_table(&vfs, item_database.clone(), npc_database.clone())
            .expect("failed to load drop table");

        GameData {
            character_creator: Box::new(DummyCharacterCreator),
            ability_value_calculator: get_ability_value_calculator(
                item_database.clone(),
                skill_database.clone(),
                npc_database.clone(),
            ),
            data_decoder: get_data_decoder(),
            drop_table,
            ai: Arc::new(get_ai_database(&vfs).expect("failed to load ai database")),
            items: item_database,
            job_class: job_class_database,
            motions: Arc::new(
                get_character_motion_database(
                    &vfs,
                    &CharacterMotionDatabaseOptions {
                        load_frame_data: true,
                    },
                )
                .expect("failed to load motion database"),
            ),
            npcs: npc_database,
            products: Arc::new(
                get_product_database(&vfs).expect("failed to load product database"),
            ),
            quests: Arc::new(
                get_quest_database(&vfs, string_database.clone())
                    .expect("failed to load quest database"),
            ),
            skills: skill_database,
            status_effects: Arc::new(
                get_status_effect_database(&vfs, string_database.clone())
                    .expect("failed to load status effect database"),
            ),
            string_database,
            warp_gates: Arc::new(
                get_warp_gate_database(&vfs).expect("failed to load warp gate database"),
            ),
            zones: zone_database,
        }
    }

    fn advance_time(app: &mut App, delta: Duration) {
        let next_instant = app
            .world
            .resource::<Time>()
            .last_update()
            .unwrap_or_else(Instant::now)
            + delta;
        app.world
            .resource_mut::<Time>()
            .update_with_instant(next_instant);
    }

    struct DummyCharacterCreator;

    impl CharacterCreator for DummyCharacterCreator {
        fn create(
            &self,
            _name: String,
            _gender: CharacterGender,
            _birth_stone: u8,
            _face: u8,
            _hair: u8,
        ) -> Result<CharacterStorage, CharacterCreatorError> {
            unreachable!("test character creation is not used in command_system tests")
        }

        fn get_basic_stats(
            &self,
            _gender: CharacterGender,
        ) -> Result<BasicStats, CharacterCreatorError> {
            unreachable!("test basic stats are not used in command_system tests")
        }
    }
}
