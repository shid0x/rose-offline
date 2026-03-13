use std::time::Duration;

use bevy::{
    ecs::{
        prelude::{Entity, Query, Res, With},
        system::ParamSet,
    },
    math::Vec3Swizzles,
    time::Time,
};

use rose_data::{AbilityType, SkillData, ZoneId};
use rose_file_readers::{AipAction, AipCondition, AipSkillTarget};

use crate::game::{
    components::{
        AbilityValues, BonfireAura, BonfireAuraTier, CharacterInfo, GameClient, HealthPoints,
        ManaPoints, PartyMembership, Position,
    },
    messages::server::ServerMessage,
    GameData,
};

const BONFIRE_BASE_SKILL_ID: u16 = 1161;
const DEFAULT_BONFIRE_INTERVAL: Duration = Duration::from_secs(10);
const DEFAULT_BONFIRE_RADIUS: f32 = 600.0;
const BONFIRE_DEFAULT_SEARCH_RADIUS: f32 = 1200.0;
const BONFIRE_HP_BALANCE_MULTIPLIER: i32 = 3;
const BONFIRE_MP_BALANCE_MULTIPLIER: i32 = 3;

fn default_bonfire_tiers() -> Vec<BonfireAuraTier> {
    vec![
        BonfireAuraTier::new(
            4,
            BONFIRE_DEFAULT_SEARCH_RADIUS,
            1000.0,
            20 * BONFIRE_HP_BALANCE_MULTIPLIER,
            10 * BONFIRE_MP_BALANCE_MULTIPLIER,
        ),
        BonfireAuraTier::new(
            2,
            BONFIRE_DEFAULT_SEARCH_RADIUS,
            1000.0,
            25 * BONFIRE_HP_BALANCE_MULTIPLIER,
            15 * BONFIRE_MP_BALANCE_MULTIPLIER,
        ),
        BonfireAuraTier::new(
            0,
            BONFIRE_DEFAULT_SEARCH_RADIUS,
            1000.0,
            30 * BONFIRE_HP_BALANCE_MULTIPLIER,
            20 * BONFIRE_MP_BALANCE_MULTIPLIER,
        ),
    ]
}

#[derive(Clone, Copy)]
struct CharacterSnapshot {
    entity: Entity,
    party: Option<Entity>,
    zone_id: ZoneId,
    position: bevy::math::Vec2,
    is_alive: bool,
}

#[derive(Clone, Copy)]
struct ActiveBonfirePulse {
    owner_entity: Entity,
    owner_party: Option<Entity>,
    zone_id: ZoneId,
    position: bevy::math::Vec2,
    radius_squared: f32,
    hp_heal: i32,
    mp_heal: i32,
}

pub(crate) fn is_bonfire_skill(skill_data: &SkillData) -> bool {
    skill_data.base_skill_id.unwrap_or(skill_data.id).get() == BONFIRE_BASE_SKILL_ID
}

pub(crate) fn get_bonfire_bonus_from_skill(skill_data: &SkillData) -> (i32, i32) {
    let mut hp_heal = 0;
    let mut mp_heal = 0;

    for add_ability in skill_data.add_ability.iter().flatten() {
        match add_ability.ability_type {
            AbilityType::Health | AbilityType::RecoverHealth => {
                hp_heal = hp_heal.max(add_ability.value);
            }
            AbilityType::Mana | AbilityType::RecoverMana => {
                mp_heal = mp_heal.max(add_ability.value);
            }
            _ => {}
        }
    }

    (
        hp_heal.saturating_mul(BONFIRE_HP_BALANCE_MULTIPLIER),
        mp_heal.saturating_mul(BONFIRE_MP_BALANCE_MULTIPLIER),
    )
}

pub(crate) fn get_bonfire_effect_radius(skill_data: &SkillData) -> f32 {
    if skill_data.scope > 0 {
        skill_data.scope as f32
    } else if skill_data.cast_range > 0 {
        skill_data.cast_range as f32
    } else {
        DEFAULT_BONFIRE_RADIUS
    }
}

fn is_bonfire_target(
    entity: Entity,
    party: Option<Entity>,
    owner_entity: Entity,
    owner_party: Option<Entity>,
) -> bool {
    entity == owner_entity || (owner_party.is_some() && owner_party == party)
}

fn select_bonfire_tier(
    bonfire_position: &Position,
    bonfire_aura: &BonfireAura,
    character_snapshots: &[CharacterSnapshot],
) -> Option<BonfireAuraTier> {
    let mut tiers = bonfire_aura.tiers.iter().copied().collect::<Vec<_>>();
    tiers.sort_by_key(|tier| tier.min_nearby_allies);

    for tier in tiers.into_iter().rev() {
        let nearby_allies = character_snapshots
            .iter()
            .filter(|character| {
                character.is_alive
                    && character.zone_id == bonfire_position.zone_id
                    && is_bonfire_target(
                        character.entity,
                        character.party,
                        bonfire_aura.owner_entity,
                        bonfire_aura.owner_party,
                    )
                    && character
                        .position
                        .distance_squared(bonfire_position.position.xy())
                        <= tier.search_radius * tier.search_radius
            })
            .count();

        if nearby_allies >= tier.min_nearby_allies {
            return Some(tier);
        }
    }

    None
}

fn build_bonfire_aura_data(
    game_data: &GameData,
    skill_data: &SkillData,
) -> (Duration, Vec<BonfireAuraTier>) {
    let Some(summon_npc_id) = skill_data.summon_npc_id else {
        return (DEFAULT_BONFIRE_INTERVAL, Vec::new());
    };
    let Some(summon_npc_data) = game_data.npcs.get_npc(summon_npc_id) else {
        return (DEFAULT_BONFIRE_INTERVAL, Vec::new());
    };
    let Some(ai_program) = game_data.ai.get_ai(summon_npc_data.ai_file_index as usize) else {
        return (DEFAULT_BONFIRE_INTERVAL, Vec::new());
    };

    let mut tiers = Vec::new();

    if let Some(trigger_on_idle) = ai_program.trigger_on_idle.as_ref() {
        for event in &trigger_on_idle.events {
            let Some(support_skill_id) = event.actions.iter().find_map(|action| match action {
                AipAction::UseSkill(AipSkillTarget::This, skill_id, _) => {
                    rose_data::SkillId::new(*skill_id as u16)
                }
                _ => None,
            }) else {
                continue;
            };

            let Some(support_skill_data) = game_data.skills.get_skill(support_skill_id) else {
                continue;
            };

            let (hp_heal, mp_heal) = get_bonfire_bonus_from_skill(support_skill_data);
            if hp_heal == 0 && mp_heal == 0 {
                continue;
            }

            let effect_radius = get_bonfire_effect_radius(support_skill_data);
            let (min_nearby_allies, search_radius) = event
                .conditions
                .iter()
                .find_map(|condition| match condition {
                    AipCondition::FindNearbyEntities(find_nearby_entities)
                        if find_nearby_entities.is_allied
                            && find_nearby_entities.count_operator_type.is_none() =>
                    {
                        Some((
                            find_nearby_entities.count.max(0) as usize,
                            find_nearby_entities.distance as f32,
                        ))
                    }
                    _ => None,
                })
                .unwrap_or((0, effect_radius));

            tiers.push(BonfireAuraTier::new(
                min_nearby_allies,
                search_radius,
                effect_radius,
                hp_heal,
                mp_heal,
            ));
        }
    }

    (ai_program.idle_trigger_interval, tiers)
}

pub(crate) fn create_bonfire_aura(
    game_data: &GameData,
    skill_data: &SkillData,
    owner_entity: Entity,
    owner_party: Option<Entity>,
) -> BonfireAura {
    let (pulse_interval, mut tiers) = build_bonfire_aura_data(game_data, skill_data);
    if tiers.is_empty() {
        tiers = default_bonfire_tiers();
    }

    let mut bonfire_aura = BonfireAura::new(
        owner_entity,
        owner_party,
        if pulse_interval.is_zero() {
            DEFAULT_BONFIRE_INTERVAL
        } else {
            pulse_interval
        },
        tiers,
    );
    bonfire_aura.pulse_immediately();
    bonfire_aura
}

pub fn bonfire_aura_system(
    time: Res<Time>,
    mut bonfire_query: Query<(&Position, &mut BonfireAura)>,
    owner_query: Query<&PartyMembership>,
    mut character_queries: ParamSet<(
        Query<(Entity, &PartyMembership, &Position, &HealthPoints), With<CharacterInfo>>,
        Query<
            (
                Entity,
                &PartyMembership,
                &Position,
                &AbilityValues,
                &mut HealthPoints,
                &mut ManaPoints,
                Option<&GameClient>,
            ),
            With<CharacterInfo>,
        >,
    )>,
) {
    let character_snapshots = character_queries
        .p0()
        .iter()
        .map(
            |(entity, party_membership, position, health_points)| CharacterSnapshot {
                entity,
                party: party_membership.party,
                zone_id: position.zone_id,
                position: position.position.xy(),
                is_alive: health_points.hp > 0,
            },
        )
        .collect::<Vec<_>>();

    let mut active_pulses = Vec::new();

    for (position, mut bonfire_aura) in bonfire_query.iter_mut() {
        bonfire_aura.owner_party = owner_query
            .get(bonfire_aura.owner_entity)
            .ok()
            .and_then(|party_membership| party_membership.party);

        bonfire_aura.elapsed += time.delta();
        if bonfire_aura.elapsed < bonfire_aura.pulse_interval {
            continue;
        }
        bonfire_aura.elapsed = bonfire_aura
            .elapsed
            .saturating_sub(bonfire_aura.pulse_interval);

        let Some(selected_tier) =
            select_bonfire_tier(position, &bonfire_aura, &character_snapshots)
        else {
            continue;
        };

        active_pulses.push(ActiveBonfirePulse {
            owner_entity: bonfire_aura.owner_entity,
            owner_party: bonfire_aura.owner_party,
            zone_id: position.zone_id,
            position: position.position.xy(),
            radius_squared: selected_tier.effect_radius * selected_tier.effect_radius,
            hp_heal: selected_tier.hp_bonus,
            mp_heal: selected_tier.mp_bonus,
        });
    }

    if active_pulses.is_empty() {
        return;
    }

    for (
        entity,
        party_membership,
        position,
        ability_values,
        mut health_points,
        mut mana_points,
        game_client,
    ) in character_queries.p1().iter_mut()
    {
        if health_points.hp <= 0 {
            continue;
        }

        let mut hp_heal = 0;
        let mut mp_heal = 0;

        for pulse in active_pulses.iter() {
            if position.zone_id != pulse.zone_id {
                continue;
            }

            if position.position.xy().distance_squared(pulse.position) > pulse.radius_squared {
                continue;
            }

            if !is_bonfire_target(
                entity,
                party_membership.party,
                pulse.owner_entity,
                pulse.owner_party,
            ) {
                continue;
            }

            hp_heal += pulse.hp_heal;
            mp_heal += pulse.mp_heal;
        }

        if hp_heal == 0 && mp_heal == 0 {
            continue;
        }

        let new_hp = i32::min(health_points.hp + hp_heal, ability_values.get_max_health());
        let new_mp = i32::min(mana_points.mp + mp_heal, ability_values.get_max_mana());

        let hp_changed = new_hp != health_points.hp;
        let mp_changed = new_mp != mana_points.mp;

        if !hp_changed && !mp_changed {
            continue;
        }

        health_points.hp = new_hp;
        mana_points.mp = new_mp;

        if let Some(game_client) = game_client {
            if hp_changed {
                game_client
                    .server_message_tx
                    .send(ServerMessage::UpdateAbilityValueSet {
                        ability_type: AbilityType::Health,
                        value: new_hp,
                    })
                    .ok();
            }

            if mp_changed {
                game_client
                    .server_message_tx
                    .send(ServerMessage::UpdateAbilityValueSet {
                        ability_type: AbilityType::Mana,
                        value: new_mp,
                    })
                    .ok();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroUsize, time::Duration};

    use arrayvec::ArrayVec;
    use bevy::{math::Vec3, math::Vec3Swizzles, prelude::Entity};
    use rose_data::{
        EffectFileId, EffectId, ItemClass, JobClassId, MotionId, NpcId, SkillActionMode,
        SkillCastingEffect, SkillCooldown, SkillData, SkillId, SkillTargetFilter, SkillType,
        SoundId, StatusEffectId, ZoneId,
    };

    use super::{is_bonfire_skill, select_bonfire_tier, CharacterSnapshot};
    use crate::game::components::{BonfireAura, BonfireAuraTier, Position};

    fn test_skill_data() -> SkillData {
        SkillData {
            id: SkillId::new(1161).unwrap(),
            name: "",
            description: "",
            base_skill_id: None,
            level: 1,
            learn_point_cost: 0,
            learn_money_cost: 0,
            skill_type: SkillType::SummonPet,
            page: 0,
            icon_number: 0,
            use_ability: ArrayVec::new(),
            required_ability: ArrayVec::new(),
            required_job_class: JobClassId::new(1),
            required_planet: NonZeroUsize::new(1),
            required_skills: ArrayVec::new(),
            required_union: ArrayVec::new(),
            required_equipment_class: ArrayVec::<ItemClass, 5>::new(),
            action_mode: SkillActionMode::Stop,
            action_motion_id: Some(MotionId::new(1)),
            action_motion_speed: 1.0,
            add_ability: [None, None],
            basic_command: None,
            bullet_effect_id: EffectId::new(1),
            bullet_link_dummy_bone_id: 0,
            bullet_fire_sound_id: SoundId::new(1),
            cast_range: 0,
            casting_motion_id: Some(MotionId::new(1)),
            casting_motion_speed: 1.0,
            casting_repeat_motion_id: Some(MotionId::new(1)),
            casting_repeat_motion_count: 1,
            casting_effects: [
                Some(SkillCastingEffect {
                    effect_file_id: EffectFileId::new(1).unwrap(),
                    effect_dummy_bone_id: None,
                }),
                None,
                None,
                None,
            ],
            cooldown: SkillCooldown::Skill {
                duration: Duration::from_secs(1),
            },
            damage_type: 0,
            harm: 0,
            hit_effect_file_id: EffectFileId::new(1),
            hit_link_dummy_bone_id: None,
            hit_sound_id: SoundId::new(1),
            hit_dummy_effect_file_id: [None, None],
            hit_dummy_sound_id: [None, None],
            item_make_number: 0,
            power: 0,
            scope: 0,
            status_effects: [StatusEffectId::new(1), None],
            status_effect_duration: Duration::from_secs(1),
            success_ratio: 0,
            summon_npc_id: NpcId::new(1),
            target_filter: SkillTargetFilter::OnlySelf,
            warp_zone_id: ZoneId::new(1),
            warp_zone_x: 0.0,
            warp_zone_y: 0.0,
        }
    }

    #[test]
    fn bonfire_skill_family_matches_upgrades() {
        let mut skill_data = test_skill_data();
        assert!(is_bonfire_skill(&skill_data));

        skill_data.id = SkillId::new(1166).unwrap();
        skill_data.base_skill_id = SkillId::new(1161);
        assert!(is_bonfire_skill(&skill_data));
    }

    #[test]
    fn bonfire_selects_stronger_tier_for_smaller_party() {
        let owner_entity = Entity::from_raw(1);
        let party_entity = Entity::from_raw(2);
        let position = Position::new(Vec3::ZERO, ZoneId::new(1).unwrap());
        let bonfire_aura = BonfireAura::new(
            owner_entity,
            Some(party_entity),
            Duration::from_secs(10),
            vec![
                BonfireAuraTier::new(0, 1000.0, 1000.0, 30, 20),
                BonfireAuraTier::new(2, 1200.0, 1000.0, 25, 15),
                BonfireAuraTier::new(4, 1200.0, 1000.0, 20, 10),
            ],
        );

        let solo = vec![CharacterSnapshot {
            entity: owner_entity,
            party: Some(party_entity),
            zone_id: ZoneId::new(1).unwrap(),
            position: Vec3::ZERO.xy(),
            is_alive: true,
        }];
        assert_eq!(
            select_bonfire_tier(&position, &bonfire_aura, &solo)
                .unwrap()
                .hp_bonus,
            30
        );

        let group = vec![
            CharacterSnapshot {
                entity: owner_entity,
                party: Some(party_entity),
                zone_id: ZoneId::new(1).unwrap(),
                position: Vec3::ZERO.xy(),
                is_alive: true,
            },
            CharacterSnapshot {
                entity: Entity::from_raw(3),
                party: Some(party_entity),
                zone_id: ZoneId::new(1).unwrap(),
                position: Vec3::new(10.0, 0.0, 0.0).xy(),
                is_alive: true,
            },
            CharacterSnapshot {
                entity: Entity::from_raw(4),
                party: Some(party_entity),
                zone_id: ZoneId::new(1).unwrap(),
                position: Vec3::new(20.0, 0.0, 0.0).xy(),
                is_alive: true,
            },
            CharacterSnapshot {
                entity: Entity::from_raw(5),
                party: Some(party_entity),
                zone_id: ZoneId::new(1).unwrap(),
                position: Vec3::new(30.0, 0.0, 0.0).xy(),
                is_alive: true,
            },
        ];
        assert_eq!(
            select_bonfire_tier(&position, &bonfire_aura, &group)
                .unwrap()
                .hp_bonus,
            20
        );
    }
}
