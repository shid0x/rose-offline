use std::marker::PhantomData;

use bevy::{
    ecs::{
        prelude::{Commands, Entity, EventReader, EventWriter, Local, Query, Res, ResMut},
        query::WorldQuery,
        system::SystemParam,
    },
    math::Vec3Swizzles,
    time::Time,
};
use log::warn;
use rand::Rng;

use rose_data::{
    AbilityType, SkillCooldown, SkillData, SkillTargetFilter, SkillType, StatusEffectClearedByType,
    StatusEffectType,
};
use rose_game_common::{components::Money, data::Damage};

use super::bonfire_aura_system::{create_bonfire_aura, is_bonfire_skill};
use crate::game::{
    bundles::{ability_values_get_value, MonsterBundle, GLOBAL_SKILL_COOLDOWN},
    components::{
        AbilityValues, ClanMembership, ClientEntity, ClientEntityType, Command, Cooldowns, Dead,
        ExperiencePoints, GameClient, HealthPoints, Inventory, Level, ManaPoints, MoveMode,
        MoveSpeed, NextCommand, PartyMembership, Position, SpawnOrigin, Stamina, StatusEffects,
        SummonPointCost, SummonUsage, Team,
    },
    events::{DamageEvent, ItemLifeEvent, SkillEvent, SkillEventTarget},
    messages::server::{CancelCastingSkillReason, ServerMessage},
    pvp::can_character_attack_character,
    resources::{ClientEntityList, ServerMessages},
    GameData,
};

#[allow(dead_code)]
enum SkillCastError {
    InvalidSkill,
    InvalidTarget,
    NotEnoughUseAbility,
    NotEnoughSummonPoints,
}

#[derive(SystemParam)]
pub struct SkillSystemParameters<'w, 's> {
    server_messages: ResMut<'w, ServerMessages>,
    damage_events: EventWriter<'w, DamageEvent>,
    item_life_events: EventWriter<'w, ItemLifeEvent>,

    #[system_param(ignore)]
    _secret: PhantomData<&'s ()>,
}

#[derive(SystemParam)]
pub struct SkillSystemResources<'w, 's> {
    game_data: Res<'w, GameData>,
    zone_list: Res<'w, crate::game::resources::ZoneList>,
    time: Res<'w, Time>,

    #[system_param(ignore)]
    _secret: PhantomData<&'s ()>,
}

#[derive(WorldQuery)]
#[world_query(mutable)]
pub struct SkillCasterQuery<'w> {
    entity: Entity,

    ability_values: &'w AbilityValues,
    client_entity: &'w ClientEntity,
    level: &'w Level,
    move_mode: &'w MoveMode,
    position: &'w Position,
    team: &'w Team,

    clan_membership: Option<&'w ClanMembership>,
    game_client: Option<&'w GameClient>,
    party_membership: Option<&'w PartyMembership>,

    experience_points: Option<&'w mut ExperiencePoints>,
    cooldowns: Option<&'w mut Cooldowns>,
    inventory: Option<&'w mut Inventory>,
    summon_usage: Option<&'w mut SummonUsage>,
}

#[derive(WorldQuery)]
#[world_query(mutable)]
pub struct SkillTargetQuery<'w> {
    entity: Entity,

    ability_values: &'w AbilityValues,
    client_entity: &'w ClientEntity,
    level: &'w Level,
    move_speed: &'w MoveSpeed,
    position: &'w Position,
    team: &'w Team,

    clan_membership: Option<&'w ClanMembership>,
    dead: Option<&'w Dead>,
    party_membership: Option<&'w PartyMembership>,

    health_points: &'w mut HealthPoints,
    mana_points: Option<&'w mut ManaPoints>,
    command: &'w mut Command,
    next_command: &'w mut NextCommand,
    stamina: Option<&'w mut Stamina>,
    status_effects: &'w mut StatusEffects,
}

fn stop_action_disabled_target(
    server_messages: &mut ServerMessages,
    client_entity: &ClientEntity,
    position: &Position,
    command: &mut Command,
    next_command: &mut NextCommand,
) {
    let should_send_stop = !command.is_stop() || next_command.command.is_some();

    *command = Command::with_stop();
    *next_command = NextCommand::default();

    if should_send_stop {
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
}

fn stop_skill_use_disabled_target(
    server_messages: &mut ServerMessages,
    client_entity: &ClientEntity,
    position: &Position,
    command: &mut Command,
    next_command: &mut NextCommand,
) {
    let should_send_stop = matches!(
        &command.command,
        crate::game::components::CommandData::CastSkill { .. }
    );
    let should_clear_next = next_command.command.as_ref().is_some_and(|command| {
        matches!(
            command,
            crate::game::components::CommandData::CastSkill { .. }
        )
    });

    if should_send_stop {
        *command = Command::with_stop();
    }

    if should_clear_next {
        *next_command = NextCommand::default();
    }

    if should_send_stop {
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
}

fn send_skill_target_status_effects_update(
    server_messages: &mut ServerMessages,
    client_entity: &ClientEntity,
    status_effects: &StatusEffects,
) {
    server_messages.send_entity_message(
        client_entity,
        ServerMessage::UpdateStatusEffects {
            entity_id: client_entity.id,
            status_effects: status_effects.active.clone(),
            updated_values: Vec::new(),
        },
    );
}

// TODO: Deduplicate code with skill_use.rs check_skill_target_filter
fn check_skill_target_filter(
    game_data: &GameData,
    zone_list: &crate::game::resources::ZoneList,
    skill_caster: &SkillCasterQueryItem,
    skill_target: &SkillTargetQueryItem,
    skill_data: &SkillData,
) -> bool {
    let target_is_alive = skill_target.health_points.hp > 0;
    let target_is_caster = skill_caster.entity == skill_target.entity;

    match skill_data.target_filter {
        SkillTargetFilter::OnlySelf => target_is_alive && target_is_caster,
        SkillTargetFilter::Group => {
            let caster_party = skill_caster
                .party_membership
                .and_then(|party_membership| party_membership.party);
            let target_party = skill_target
                .party_membership
                .and_then(|party_membership| party_membership.party);
            target_is_alive
                && (target_is_caster || (caster_party.is_some() && caster_party == target_party))
        }
        SkillTargetFilter::Guild => {
            let caster_clan = skill_caster
                .clan_membership
                .and_then(|clan_membership| clan_membership.clan());
            let target_clan = skill_target
                .clan_membership
                .and_then(|clan_membership| clan_membership.clan());
            target_is_alive
                && (target_is_caster || (caster_clan.is_some() && caster_clan == target_clan))
        }
        SkillTargetFilter::Allied => {
            target_is_alive && skill_caster.team.id == skill_target.team.id
        }
        SkillTargetFilter::Monster => {
            target_is_alive
                && matches!(
                    skill_target.client_entity.entity_type,
                    ClientEntityType::Monster
                )
        }
        SkillTargetFilter::Enemy => {
            if target_is_alive
                && matches!(
                    skill_caster.client_entity.entity_type,
                    ClientEntityType::Character
                )
                && matches!(
                    skill_target.client_entity.entity_type,
                    ClientEntityType::Character
                )
            {
                if target_is_caster {
                    return false;
                }

                if skill_caster.position.zone_id != skill_target.position.zone_id {
                    return false;
                }

                let Some(zone_data) = game_data.zones.get_zone(skill_caster.position.zone_id)
                else {
                    return false;
                };

                return can_character_attack_character(
                    zone_data,
                    zone_list.get_pvp_enabled(skill_caster.position.zone_id),
                    skill_caster.team.id,
                    skill_target.team.id,
                );
            }

            target_is_alive
                && skill_target.team.id != Team::DEFAULT_NPC_TEAM_ID
                && skill_caster.team.id != skill_target.team.id
        }
        SkillTargetFilter::EnemyCharacter => {
            if target_is_alive
                && matches!(
                    skill_caster.client_entity.entity_type,
                    ClientEntityType::Character
                )
                && matches!(
                    skill_target.client_entity.entity_type,
                    ClientEntityType::Character
                )
            {
                if target_is_caster {
                    return false;
                }

                if skill_caster.position.zone_id != skill_target.position.zone_id {
                    return false;
                }

                let Some(zone_data) = game_data.zones.get_zone(skill_caster.position.zone_id)
                else {
                    return false;
                };

                return can_character_attack_character(
                    zone_data,
                    zone_list.get_pvp_enabled(skill_caster.position.zone_id),
                    skill_caster.team.id,
                    skill_target.team.id,
                );
            }

            target_is_alive
                && skill_caster.team.id != skill_target.team.id
                && matches!(
                    skill_target.client_entity.entity_type,
                    ClientEntityType::Character
                )
        }
        SkillTargetFilter::Character => {
            target_is_alive
                && matches!(
                    skill_target.client_entity.entity_type,
                    ClientEntityType::Character
                )
        }
        SkillTargetFilter::CharacterOrMonster => {
            target_is_alive
                && matches!(
                    skill_target.client_entity.entity_type,
                    ClientEntityType::Character | ClientEntityType::Monster
                )
        }
        SkillTargetFilter::DeadAlliedCharacter => {
            !target_is_alive
                && !target_is_caster
                && skill_caster.team.id == skill_target.team.id
                && matches!(
                    skill_target.client_entity.entity_type,
                    ClientEntityType::Character
                )
        }
        SkillTargetFilter::EnemyMonster => {
            target_is_alive
                && skill_caster.team.id != skill_target.team.id
                && matches!(
                    skill_target.client_entity.entity_type,
                    ClientEntityType::Monster
                )
        }
    }
}

fn apply_skill_status_effects_to_entity(
    skill_system_parameters: &mut SkillSystemParameters,
    skill_system_resources: &SkillSystemResources,
    skill_caster: &SkillCasterQueryItem,
    skill_target: &mut SkillTargetQueryItem,
    skill_data: &SkillData,
) -> Result<(), SkillCastError> {
    if !check_skill_target_filter(
        &skill_system_resources.game_data,
        &skill_system_resources.zone_list,
        skill_caster,
        skill_target,
        skill_data,
    ) {
        return Err(SkillCastError::InvalidTarget);
    }

    if skill_data.harm != 0 {
        skill_system_parameters
            .damage_events
            .send(DamageEvent::Tagged {
                attacker: skill_caster.entity,
                defender: skill_target.entity,
            });
    }

    let mut effect_success = [false, false];
    let mut status_effects_updated = false;
    for (effect_index, status_effect_data) in skill_data
        .status_effects
        .iter()
        .enumerate()
        .filter_map(|(index, id)| {
            id.and_then(|id| {
                skill_system_resources
                    .game_data
                    .status_effects
                    .get_status_effect(id)
            })
            .map(|id| (index, id))
        })
    {
        if skill_data.success_ratio > 0 {
            match status_effect_data.cleared_by_type {
                StatusEffectClearedByType::ClearGood => {
                    if skill_data.success_ratio
                        < skill_target.level.level as i32 - skill_caster.level.level as i32
                            + rand::thread_rng().gen_range(1..=100)
                    {
                        continue;
                    }
                }
                _ => {
                    if skill_data.success_ratio as f32
                        * (skill_caster.level.level as i32 * 2
                            + skill_caster.ability_values.get_intelligence()
                            + 20) as f32
                        / (skill_target.ability_values.get_resistance() as f32 * 0.6
                            + 5.0
                            + skill_target.ability_values.get_avoid() as f32)
                        <= rand::thread_rng().gen_range(1..=100) as f32
                    {
                        continue;
                    }
                }
            }
        }

        let adjust_value = if matches!(
            status_effect_data.status_effect_type,
            StatusEffectType::AdditionalDamageRate
        ) {
            skill_data.power as i32
        } else if let Some(skill_add_ability) = skill_data.add_ability[effect_index].as_ref() {
            let ability_value = ability_values_get_value(
                skill_add_ability.ability_type,
                Some(skill_target.ability_values),
                Some(skill_target.level),
                Some(skill_target.move_speed),
                Some(skill_target.team),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap_or(0);

            skill_system_resources
                .game_data
                .ability_value_calculator
                .calculate_skill_adjust_value(
                    skill_add_ability,
                    skill_caster.ability_values.get_intelligence(),
                    ability_value,
                )
        } else {
            0
        };

        if skill_target
            .status_effects
            .can_apply(status_effect_data, adjust_value)
        {
            status_effects_updated |= skill_target.status_effects.apply_status_effect(
                status_effect_data,
                skill_system_resources.time.last_update().unwrap()
                    + skill_data.status_effect_duration,
                adjust_value,
            );

            match status_effect_data.status_effect_type {
                StatusEffectType::Dumb => {
                    stop_skill_use_disabled_target(
                        &mut skill_system_parameters.server_messages,
                        skill_target.client_entity,
                        skill_target.position,
                        &mut skill_target.command,
                        &mut skill_target.next_command,
                    );
                }
                StatusEffectType::Fainting | StatusEffectType::Sleep => {
                    stop_action_disabled_target(
                        &mut skill_system_parameters.server_messages,
                        skill_target.client_entity,
                        skill_target.position,
                        &mut skill_target.command,
                        &mut skill_target.next_command,
                    );
                }
                StatusEffectType::Taunt => {
                    // TODO: Set current + next command to attack spell cast entity
                }
                _ => {}
            }

            effect_success[effect_index] = true;
        }
    }

    for (effect_index, add_ability) in
        skill_data
            .add_ability
            .iter()
            .enumerate()
            .filter_map(|(index, add_ability)| {
                add_ability.as_ref().map(|add_ability| (index, add_ability))
            })
    {
        match add_ability.ability_type {
            AbilityType::Health => {
                skill_target.health_points.hp = i32::min(
                    skill_target.ability_values.get_max_health(),
                    skill_target.health_points.hp
                        + skill_system_resources
                            .game_data
                            .ability_value_calculator
                            .calculate_skill_adjust_value(
                                add_ability,
                                skill_caster.ability_values.get_intelligence(),
                                skill_target.health_points.hp,
                            ),
                );
                effect_success[effect_index] = true;
            }
            AbilityType::Mana => {
                if let Some(target_mana_points) = skill_target.mana_points.as_mut() {
                    target_mana_points.mp = i32::min(
                        skill_target.ability_values.get_max_mana(),
                        target_mana_points.mp + add_ability.value,
                    );
                }
                effect_success[effect_index] = true;
            }
            AbilityType::Stamina | AbilityType::Money => {
                warn!(
                    "Unimplemented skill status effect add ability_type {:?}, value {}",
                    add_ability.ability_type, add_ability.value
                )
            }
            _ => {}
        }
    }

    if effect_success.iter().any(|x| *x) {
        skill_system_parameters.server_messages.send_entity_message(
            skill_target.client_entity,
            ServerMessage::ApplySkillEffect {
                entity_id: skill_target.client_entity.id,
                caster_entity_id: skill_caster.client_entity.id,
                caster_intelligence: skill_caster.ability_values.get_intelligence(),
                skill_id: skill_data.id,
                effect_success,
            },
        );

        if status_effects_updated {
            send_skill_target_status_effects_update(
                &mut skill_system_parameters.server_messages,
                skill_target.client_entity,
                &skill_target.status_effects,
            );
        }
    }

    Ok(())
}

fn apply_skill_status_effects(
    skill_system_parameters: &mut SkillSystemParameters,
    skill_system_resources: &SkillSystemResources,
    client_entity_list: &ClientEntityList,
    skill_caster: &SkillCasterQueryItem,
    skill_target: &SkillEventTarget,
    skill_data: &SkillData,
    skill_target_query: &mut Query<SkillTargetQuery>,
) -> Result<(), SkillCastError> {
    if skill_data.scope > 0 {
        // Apply in AOE around target position
        let client_entity_zone = client_entity_list
            .get_zone(skill_caster.position.zone_id)
            .ok_or(SkillCastError::InvalidTarget)?;

        let skill_position = match *skill_target {
            SkillEventTarget::Entity(target_entity) => {
                if let Ok(skill_target) = skill_target_query.get_mut(target_entity) {
                    Some(skill_target.position.position.xy())
                } else {
                    None
                }
            }
            SkillEventTarget::Position(position) => Some(position),
        }
        .ok_or(SkillCastError::InvalidTarget)?;

        for (target_entity, _) in client_entity_zone
            .iter_entities_within_distance(skill_position, skill_data.scope as f32)
        {
            if let Ok(mut skill_target) = skill_target_query.get_mut(target_entity) {
                apply_skill_status_effects_to_entity(
                    skill_system_parameters,
                    skill_system_resources,
                    skill_caster,
                    &mut skill_target,
                    skill_data,
                )
                .ok();
            }
        }

        Ok(())
    } else if let SkillEventTarget::Entity(target_entity) = *skill_target {
        if let Ok(mut skill_target) = skill_target_query.get_mut(target_entity) {
            apply_skill_status_effects_to_entity(
                skill_system_parameters,
                skill_system_resources,
                skill_caster,
                &mut skill_target,
                skill_data,
            )
            .ok();
            Ok(())
        } else {
            Err(SkillCastError::InvalidTarget)
        }
    } else {
        Err(SkillCastError::InvalidTarget)
    }
}

#[cfg(test)]
mod tests {
    use bevy::{math::Vec3, prelude::Entity};
    use rose_data::{StatusEffectId, StatusEffectType, ZoneId};

    use super::{
        send_skill_target_status_effects_update, stop_action_disabled_target,
        stop_skill_use_disabled_target,
    };
    use crate::game::{
        components::{
            ActiveStatusEffect, ClientEntity, ClientEntityId, ClientEntityType, Command,
            CommandData, NextCommand, Position, StatusEffects,
        },
        messages::server::ServerMessage,
        resources::ServerMessages,
    };
    use rose_game_common::components::MoveMode;
    use std::time::Duration;

    #[test]
    fn stop_action_disabled_target_clears_command_and_queue_and_notifies_client() {
        let mut server_messages = ServerMessages::default();
        let client_entity = ClientEntity::new(
            ClientEntityType::Monster,
            ClientEntityId(99),
            ZoneId::new(1).unwrap(),
        );
        let position = Position::new(Vec3::new(10.0, 20.0, 30.0), ZoneId::new(1).unwrap());
        let mut command =
            Command::with_move(Vec3::new(100.0, 200.0, 30.0), None, Some(MoveMode::Run));
        let mut next_command = NextCommand::with_attack(Entity::from_raw(7));

        stop_action_disabled_target(
            &mut server_messages,
            &client_entity,
            &position,
            &mut command,
            &mut next_command,
        );

        assert!(command.is_stop());
        assert!(next_command.command.is_none());
        assert_eq!(server_messages.pending_entity_messages.len(), 1);

        match &server_messages.pending_entity_messages[0].message {
            ServerMessage::StopMoveEntity { entity_id, x, y, z } => {
                assert_eq!(*entity_id, client_entity.id);
                assert_eq!(*x, position.position.x);
                assert_eq!(*y, position.position.y);
                assert_eq!(*z, position.position.z as u16);
            }
            other => panic!("expected StopMoveEntity, got {:?}", other),
        }
    }

    #[test]
    fn stop_action_disabled_target_is_noop_for_already_stopped_targets() {
        let mut server_messages = ServerMessages::default();
        let client_entity = ClientEntity::new(
            ClientEntityType::Monster,
            ClientEntityId(100),
            ZoneId::new(1).unwrap(),
        );
        let position = Position::new(Vec3::new(1.0, 2.0, 3.0), ZoneId::new(1).unwrap());
        let mut command = Command::with_stop();
        let mut next_command = NextCommand::default();

        stop_action_disabled_target(
            &mut server_messages,
            &client_entity,
            &position,
            &mut command,
            &mut next_command,
        );

        assert!(command.is_stop());
        assert!(next_command.command.is_none());
        assert!(server_messages.pending_entity_messages.is_empty());
    }

    #[test]
    fn stop_skill_use_disabled_target_clears_only_skill_commands() {
        let mut server_messages = ServerMessages::default();
        let client_entity = ClientEntity::new(
            ClientEntityType::Monster,
            ClientEntityId(102),
            ZoneId::new(1).unwrap(),
        );
        let position = Position::new(Vec3::new(1.0, 2.0, 3.0), ZoneId::new(1).unwrap());
        let mut command =
            Command::with_move(Vec3::new(100.0, 200.0, 30.0), None, Some(MoveMode::Run));
        let mut next_command = NextCommand::with_attack(Entity::from_raw(7));

        stop_skill_use_disabled_target(
            &mut server_messages,
            &client_entity,
            &position,
            &mut command,
            &mut next_command,
        );

        assert!(matches!(command.command, CommandData::Move { .. }));
        assert!(matches!(
            next_command.command,
            Some(CommandData::Attack { .. })
        ));
        assert!(server_messages.pending_entity_messages.is_empty());
    }

    #[test]
    fn stop_skill_use_disabled_target_stops_active_cast_and_clears_next_cast() {
        let mut server_messages = ServerMessages::default();
        let client_entity = ClientEntity::new(
            ClientEntityType::Monster,
            ClientEntityId(103),
            ZoneId::new(1).unwrap(),
        );
        let position = Position::new(Vec3::new(4.0, 5.0, 6.0), ZoneId::new(1).unwrap());
        let mut command = Command::with_cast_skill(
            rose_data::SkillId::new(1).unwrap(),
            None,
            Duration::ZERO,
            Duration::ZERO,
        );
        let mut next_command =
            NextCommand::with_cast_skill_target_self(rose_data::SkillId::new(2).unwrap(), None);

        stop_skill_use_disabled_target(
            &mut server_messages,
            &client_entity,
            &position,
            &mut command,
            &mut next_command,
        );

        assert!(command.is_stop());
        assert!(next_command.command.is_none());
        assert_eq!(server_messages.pending_entity_messages.len(), 1);
    }

    #[test]
    fn send_skill_target_status_effects_update_queues_authoritative_status_state() {
        let mut server_messages = ServerMessages::default();
        let client_entity = ClientEntity::new(
            ClientEntityType::Character,
            ClientEntityId(101),
            ZoneId::new(1).unwrap(),
        );
        let mut status_effects = StatusEffects::default();
        status_effects.active[StatusEffectType::DecreaseMoveSpeed] = Some(ActiveStatusEffect {
            id: StatusEffectId::new(7).unwrap(),
            value: 42,
        });

        send_skill_target_status_effects_update(
            &mut server_messages,
            &client_entity,
            &status_effects,
        );

        assert_eq!(server_messages.pending_entity_messages.len(), 1);
        match &server_messages.pending_entity_messages[0].message {
            ServerMessage::UpdateStatusEffects {
                entity_id,
                status_effects,
                updated_values,
            } => {
                assert_eq!(*entity_id, client_entity.id);
                let slow_effect = status_effects[StatusEffectType::DecreaseMoveSpeed]
                    .as_ref()
                    .expect("slow should be present");
                assert_eq!(slow_effect.id, StatusEffectId::new(7).unwrap());
                assert_eq!(slow_effect.value, 42);
                assert!(updated_values.is_empty());
            }
            other => panic!("expected UpdateStatusEffects, got {:?}", other),
        }
    }
}

fn apply_skill_damage_to_entity(
    skill_system_parameters: &mut SkillSystemParameters,
    skill_system_resources: &SkillSystemResources,
    skill_caster: &SkillCasterQueryItem,
    skill_target: &mut SkillTargetQueryItem,
    skill_data: &SkillData,
) -> Result<Damage, SkillCastError> {
    if !check_skill_target_filter(
        &skill_system_resources.game_data,
        &skill_system_resources.zone_list,
        skill_caster,
        skill_target,
        skill_data,
    ) {
        return Err(SkillCastError::InvalidTarget);
    }

    // TODO: Get hit count from skill action motion
    let damage = skill_system_resources
        .game_data
        .ability_value_calculator
        .calculate_skill_damage(
            skill_caster.ability_values,
            skill_target.ability_values,
            skill_data,
            1,
        );

    skill_system_parameters
        .damage_events
        .send(DamageEvent::Skill {
            attacker: skill_caster.entity,
            defender: skill_target.entity,
            damage,
            skill_id: skill_data.id,
            attacker_intelligence: skill_caster.ability_values.get_intelligence(),
        });

    Ok(damage)
}

fn apply_skill_damage(
    skill_system_parameters: &mut SkillSystemParameters,
    skill_system_resources: &SkillSystemResources,
    client_entity_list: &ClientEntityList,
    skill_caster: &SkillCasterQueryItem,
    skill_target: &SkillEventTarget,
    skill_data: &SkillData,
    skill_target_query: &mut Query<SkillTargetQuery>,
) -> Result<(), SkillCastError> {
    let result = if skill_data.scope > 0 {
        // Apply in AOE around target position
        let client_entity_zone = client_entity_list
            .get_zone(skill_caster.position.zone_id)
            .ok_or(SkillCastError::InvalidTarget)?;

        let skill_position = match *skill_target {
            SkillEventTarget::Entity(target_entity) => {
                if let Ok(skill_target) = skill_target_query.get_mut(target_entity) {
                    Some(skill_target.position.position.xy())
                } else {
                    None
                }
            }
            SkillEventTarget::Position(position) => Some(position),
        }
        .ok_or(SkillCastError::InvalidTarget)?;

        for (target_entity, _) in client_entity_zone
            .iter_entities_within_distance(skill_position, skill_data.scope as f32)
        {
            if let Ok(mut skill_target) = skill_target_query.get_mut(target_entity) {
                apply_skill_damage_to_entity(
                    skill_system_parameters,
                    skill_system_resources,
                    skill_caster,
                    &mut skill_target,
                    skill_data,
                )
                .ok();
            }
        }

        Ok(())
    } else if let SkillEventTarget::Entity(target_entity) = *skill_target {
        // Apply directly to entity
        if let Ok(mut skill_target) = skill_target_query.get_mut(target_entity) {
            apply_skill_damage_to_entity(
                skill_system_parameters,
                skill_system_resources,
                skill_caster,
                &mut skill_target,
                skill_data,
            )
            .ok();
            Ok(())
        } else {
            Err(SkillCastError::InvalidTarget)
        }
    } else {
        Err(SkillCastError::InvalidTarget)
    };

    if result.is_ok() && skill_data.damage_type != 3 {
        skill_system_parameters
            .item_life_events
            .send(ItemLifeEvent::DecreaseWeaponLife {
                entity: skill_caster.entity,
            });
    }

    result
}

fn subtract_skill_use_cost(
    skill_system_resources: &SkillSystemResources,
    skill_caster_query: &mut Query<SkillCasterQuery>,
    skill_target_query: &mut Query<SkillTargetQuery>,
    skill_system_parameters: &mut SkillSystemParameters,
    skill_event: &SkillEvent,
) {
    // Immediately subtract skill use cost, we do not need to check requirements here
    // as that has already happened in command_system when starting casting skill
    let Some(skill_data) = skill_system_resources
        .game_data
        .skills
        .get_skill(skill_event.skill_id)
    else {
        return;
    };

    let Ok(mut skill_caster1) = skill_caster_query.get_mut(skill_event.caster_entity) else {
        return;
    };

    let Ok(mut skill_caster2) = skill_target_query.get_mut(skill_event.caster_entity) else {
        return;
    };

    if let Some(mut cooldowns) = skill_caster1.cooldowns {
        let now = skill_system_resources.time.last_update().unwrap();
        cooldowns.skill_global = Some(now + GLOBAL_SKILL_COOLDOWN);

        match skill_data.cooldown {
            SkillCooldown::Skill { duration } => {
                cooldowns.skill.insert(skill_data.id, now + duration);
            }
            SkillCooldown::Group { group, duration } => {
                if let Some(group_cooldown) = cooldowns.skill_group.get_mut(group.get()) {
                    *group_cooldown = Some(now + duration);
                }
            }
        }
    }

    for &(use_ability_type, mut use_ability_value) in skill_data.use_ability.iter() {
        if use_ability_type == AbilityType::Mana {
            let use_mana_rate = (100 - skill_caster2.ability_values.get_save_mana()) as f32 / 100.0;
            use_ability_value = (use_ability_value as f32 * use_mana_rate) as i32;
        }

        match use_ability_type {
            AbilityType::Stamina => {
                if let Some(stamina) = skill_caster2.stamina.as_mut() {
                    stamina.stamina = stamina.stamina.saturating_sub(use_ability_value as u32);
                }
            }
            AbilityType::Health => {
                if skill_caster2.health_points.hp <= use_ability_value {
                    skill_caster2.health_points.hp = 1;
                } else {
                    skill_caster2.health_points.hp -= use_ability_value;
                }
            }
            AbilityType::Mana => {
                if let Some(mana_points) = skill_caster2.mana_points.as_mut() {
                    if mana_points.mp <= use_ability_value {
                        mana_points.mp = 1;
                    } else {
                        mana_points.mp -= use_ability_value;
                    }
                }
            }
            AbilityType::Experience => {
                if let Some(experience_points) = skill_caster1.experience_points.as_mut() {
                    if experience_points.xp <= use_ability_value as u64 {
                        experience_points.xp = 0;
                    } else {
                        experience_points.xp -= use_ability_value as u64;
                    }
                }
            }
            AbilityType::Money => {
                if let Some(inventory) = skill_caster1.inventory.as_mut() {
                    inventory.money = inventory.money - Money(use_ability_value as i64);
                }
            }
            AbilityType::Fuel => {
                skill_system_parameters.item_life_events.send(
                    ItemLifeEvent::DecreaseVehicleEngineLife {
                        entity: skill_event.caster_entity,
                        amount: Some(use_ability_value.clamp(0, u16::MAX as i32) as u16),
                    },
                );
            }
            _ => {}
        }
    }
}

pub fn skill_effect_system(
    mut commands: Commands,
    mut skill_system_parameters: SkillSystemParameters,
    skill_system_resources: SkillSystemResources,
    mut skill_caster_query: Query<SkillCasterQuery>,
    mut skill_target_query: Query<SkillTargetQuery>,
    mut client_entity_list: ResMut<ClientEntityList>,
    mut skill_events: EventReader<SkillEvent>,
    mut pending_skill_events: Local<Vec<SkillEvent>>,
) {
    for skill_event in skill_events.iter() {
        // Subtract the skill use cost (e.g. mana points)
        subtract_skill_use_cost(
            &skill_system_resources,
            &mut skill_caster_query,
            &mut skill_target_query,
            &mut skill_system_parameters,
            skill_event,
        );

        // Add to pending_skill_events to execute at specific time
        pending_skill_events.push(skill_event.clone());
    }

    // TODO: drain_filter pls
    let mut i = 0;
    while i != pending_skill_events.len() {
        if pending_skill_events[i].when > skill_system_resources.time.last_update().unwrap() {
            i += 1;
            continue;
        }

        let SkillEvent {
            skill_id,
            caster_entity,
            skill_target,
            use_item,
            ..
        } = pending_skill_events.remove(i);

        let Some(skill_data) = skill_system_resources.game_data.skills.get_skill(skill_id) else {
            continue;
        };

        let Ok(mut skill_caster) = skill_caster_query.get_mut(caster_entity) else {
            continue;
        };

        let mut consumed_item = None;
        let mut result = Ok(());

        // If the skill is to use an item, try take it from inventory now
        if let Some((item_slot, item)) = use_item {
            if let Some(caster_inventory) = skill_caster.inventory.as_mut() {
                if let Some(inventory_item) = caster_inventory.get_item(item_slot) {
                    if item.is_same_item(inventory_item) {
                        if let Some(item) = caster_inventory.try_take_quantity(item_slot, 1) {
                            consumed_item = Some((item_slot, item));
                        }
                    }
                }
            }

            if consumed_item.is_none() {
                // Failed to take item from inventory, cancel the skill
                result = Err(SkillCastError::NotEnoughUseAbility);
            }
        }

        if result.is_ok() {
            result = match skill_data.skill_type {
                SkillType::Immediate
                | SkillType::EnforceWeapon
                | SkillType::EnforceBullet
                | SkillType::FireBullet
                | SkillType::AreaTarget
                | SkillType::SelfDamage => {
                    match apply_skill_damage(
                        &mut skill_system_parameters,
                        &skill_system_resources,
                        &client_entity_list,
                        &skill_caster,
                        &skill_target,
                        skill_data,
                        &mut skill_target_query,
                    ) {
                        Ok(_) => apply_skill_status_effects(
                            &mut skill_system_parameters,
                            &skill_system_resources,
                            &client_entity_list,
                            &skill_caster,
                            &skill_target,
                            skill_data,
                            &mut skill_target_query,
                        ),
                        Err(err) => Err(err),
                    }
                }
                SkillType::SelfBoundDuration
                | SkillType::SelfStateDuration
                | SkillType::TargetBoundDuration
                | SkillType::TargetStateDuration
                | SkillType::SelfBound
                | SkillType::TargetBound => apply_skill_status_effects(
                    &mut skill_system_parameters,
                    &skill_system_resources,
                    &client_entity_list,
                    &skill_caster,
                    &skill_target,
                    skill_data,
                    &mut skill_target_query,
                ),
                SkillType::SelfAndTarget => {
                    // Only applies status effect if damage > 0
                    if let SkillEventTarget::Entity(target_entity) = skill_target {
                        if let Ok(mut skill_target_data) = skill_target_query.get_mut(target_entity)
                        {
                            match apply_skill_damage_to_entity(
                                &mut skill_system_parameters,
                                &skill_system_resources,
                                &skill_caster,
                                &mut skill_target_data,
                                skill_data,
                            ) {
                                Ok(damage) if damage.amount > 0 => apply_skill_status_effects(
                                    &mut skill_system_parameters,
                                    &skill_system_resources,
                                    &client_entity_list,
                                    &skill_caster,
                                    &skill_target,
                                    skill_data,
                                    &mut skill_target_query,
                                ),
                                Ok(_) => Ok(()),
                                Err(err) => Err(err),
                            }
                        } else {
                            Err(SkillCastError::InvalidTarget)
                        }
                    } else {
                        Err(SkillCastError::InvalidTarget)
                    }
                }
                SkillType::SummonPet => {
                    if let Some(npc_id) = skill_data.summon_npc_id {
                        match skill_system_resources.game_data.npcs.get_npc(npc_id) {
                            Some(summon_npc_data) => {
                                let summon_point_requirement =
                                    summon_npc_data.summon_point_requirement;
                                let used_points = skill_caster
                                    .summon_usage
                                    .as_ref()
                                    .map_or(0, |summon_usage| summon_usage.used_points);
                                let max_points =
                                    skill_caster.ability_values.get_max_summon_points();
                                if summon_point_requirement > 0
                                    && used_points.saturating_add(summon_point_requirement)
                                        > max_points
                                {
                                    Err(SkillCastError::NotEnoughSummonPoints)
                                } else if let Some(entity) = MonsterBundle::spawn(
                                    &mut commands,
                                    &mut client_entity_list,
                                    &skill_system_resources.game_data,
                                    npc_id,
                                    skill_caster.position.zone_id,
                                    SpawnOrigin::Summoned(
                                        skill_caster.entity,
                                        skill_caster.position.position,
                                    ),
                                    150,
                                    skill_caster.team.clone(),
                                    Some((skill_caster.entity, skill_caster.level)),
                                    Some(skill_data.level as i32),
                                ) {
                                    // Apply status effect to decrease summon's life over time
                                    if let Some(status_effect_data) = skill_system_resources
                                        .game_data
                                        .status_effects
                                        .get_decrease_summon_life_status_effect()
                                    {
                                        let mut status_effects = StatusEffects::new();
                                        status_effects.apply_summon_decrease_life_status_effect(
                                            status_effect_data,
                                        );
                                        commands.entity(entity).insert(status_effects);
                                    }

                                    if summon_point_requirement > 0 {
                                        if let Some(summon_usage) =
                                            skill_caster.summon_usage.as_deref_mut()
                                        {
                                            summon_usage.used_points = summon_usage
                                                .used_points
                                                .saturating_add(summon_point_requirement);
                                        }
                                        commands
                                            .entity(entity)
                                            .insert(SummonPointCost::new(summon_point_requirement));
                                    }

                                    if is_bonfire_skill(skill_data) {
                                        commands.entity(entity).insert(create_bonfire_aura(
                                            &skill_system_resources.game_data,
                                            skill_data,
                                            skill_caster.entity,
                                            skill_caster.party_membership.and_then(
                                                |party_membership| party_membership.party,
                                            ),
                                        ));
                                    }

                                    Ok(())
                                } else {
                                    Err(SkillCastError::InvalidSkill)
                                }
                            }
                            None => {
                                warn!(
                                    "Unable to summon NPC {} for skill {} because NPC data was not found",
                                    npc_id.get(),
                                    skill_data.id.get()
                                );
                                Err(SkillCastError::InvalidSkill)
                            }
                        }
                    } else {
                        Err(SkillCastError::InvalidSkill)
                    }
                }
                SkillType::BasicAction
                | SkillType::CreateWindow
                | SkillType::Passive
                | SkillType::Emote
                | SkillType::Warp => Ok(()),
                SkillType::Resurrection => {
                    warn!("Unimplemented skill type used {:?}", skill_data);
                    Ok(())
                }
            };
        }

        match result {
            Ok(_) => {
                // Send message notifying client of consumption of item
                if let Some((item_slot, _)) = consumed_item {
                    if let (Some(caster_inventory), Some(caster_game_client)) =
                        (skill_caster.inventory, skill_caster.game_client)
                    {
                        match caster_inventory.get_item(item_slot) {
                            None => {
                                // When the item has been fully consumed we send UpdateInventory packet
                                caster_game_client
                                    .server_message_tx
                                    .send(ServerMessage::UpdateInventory {
                                        items: vec![(item_slot, None)],
                                        money: None,
                                    })
                                    .ok();
                            }
                            Some(item) => {
                                // When there is still remaining quantity we send UseItem packet
                                caster_game_client
                                    .server_message_tx
                                    .send(ServerMessage::UseInventoryItem {
                                        entity_id: skill_caster.client_entity.id,
                                        item: item.get_item_reference(),
                                        inventory_slot: item_slot,
                                    })
                                    .ok();
                            }
                        }
                    }
                }

                skill_system_parameters.server_messages.send_entity_message(
                    skill_caster.client_entity,
                    ServerMessage::FinishCastingSkill {
                        entity_id: skill_caster.client_entity.id,
                        skill_id,
                    },
                )
            }
            Err(error) => {
                // Return unused item to inventory
                if let Some((item_slot, item)) = consumed_item {
                    skill_caster
                        .inventory
                        .unwrap()
                        .try_stack_with_item(item_slot, item)
                        .expect("Unexpected error returning unconsumed item to inventory");
                }

                skill_system_parameters.server_messages.send_entity_message(
                    skill_caster.client_entity,
                    ServerMessage::CancelCastingSkill {
                        entity_id: skill_caster.client_entity.id,
                        reason: match error {
                            SkillCastError::NotEnoughUseAbility => {
                                CancelCastingSkillReason::NeedAbility
                            }
                            SkillCastError::NotEnoughSummonPoints => {
                                CancelCastingSkillReason::NeedSummonPoints
                            }
                            _ => CancelCastingSkillReason::NeedTarget,
                        },
                    },
                )
            }
        }
    }
}
