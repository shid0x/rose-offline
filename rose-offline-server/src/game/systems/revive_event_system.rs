use bevy::{
    ecs::query::WorldQuery,
    prelude::{Commands, Entity, EventReader, Query, Res, ResMut, Vec3, With, Without},
};
use rand::Rng;

use rose_data::{AbilityType, ClanMemberPosition};
use rose_game_common::{
    components::{
        AbilityValues, CharacterInfo, Equipment, HealthPoints, Level, ManaPoints, MoveSpeed,
        StatusEffectsRegen, Team,
    },
    messages::server::{
        CharacterClanMembership, ServerMessage, SpawnCommandState, SpawnEntityCharacter,
    },
};

use crate::game::{
    bundles::client_entity_teleport_zone,
    components::{
        Clan, ClanMembership, ClientEntity, ClientEntitySector, ClientEntityVisibility, Command,
        DamageSources, Dead, GameClient, MoveMode, NextCommand, NpcAi, PassiveRecoveryTime,
        PersonalStore, Position, StatusEffects,
    },
    events::{ReviveEvent, RevivePosition},
    resources::{ClientEntityList, ServerMessages},
    GameData,
};

const REVIVE_SPAWN_RADIUS: f32 = 500.0;

#[derive(WorldQuery)]
#[world_query(mutable)]
pub struct ReviveEntityQuery<'w> {
    entity: Entity,

    ability_values: &'w AbilityValues,
    client_entity: &'w ClientEntity,
    client_entity_sector: &'w mut ClientEntitySector,
    character_info: &'w CharacterInfo,
    equipment: &'w Equipment,
    level: &'w Level,
    move_speed: &'w MoveSpeed,
    position: &'w Position,
    team: &'w Team,
    personal_store: Option<&'w PersonalStore>,
    clan_membership: Option<&'w ClanMembership>,

    game_client: Option<&'w GameClient>,
}

#[derive(WorldQuery)]
#[world_query(mutable)]
pub struct ReviveObserverQuery<'w> {
    entity: Entity,
    client_entity_sector: &'w ClientEntitySector,
    client_entity_visibility: &'w mut ClientEntityVisibility,
    game_client: &'w GameClient,
    position: &'w Position,
}

#[derive(WorldQuery)]
#[world_query(mutable)]
pub struct ReviveAggroQuery<'w> {
    entity: Entity,
    client_entity: Option<&'w ClientEntity>,
    position: &'w Position,
    command: &'w mut Command,
    next_command: &'w mut NextCommand,
    damage_sources: Option<&'w mut DamageSources>,
    npc_ai: Option<&'w mut NpcAi>,
}

fn queue_stop_move_entity_message(
    server_messages: &mut ServerMessages,
    client_entity: &ClientEntity,
    position: &Position,
) {
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

fn clear_same_zone_aggro_to_revived(
    revived: &ReviveEntityQueryItem,
    query_attackers: &mut Query<ReviveAggroQuery, (With<NpcAi>, Without<Dead>)>,
    server_messages: &mut ServerMessages,
) {
    for mut attacker in query_attackers.iter_mut() {
        if attacker.entity == revived.entity
            || attacker.position.zone_id != revived.position.zone_id
        {
            continue;
        }

        if attacker.command.target_entity() == Some(revived.entity) {
            *attacker.command = Command::with_stop();

            if let Some(client_entity) = attacker.client_entity {
                queue_stop_move_entity_message(server_messages, client_entity, attacker.position);
            }
        }

        if attacker.next_command.target_entity() == Some(revived.entity) {
            *attacker.next_command = NextCommand::default();
        }

        if let Some(mut damage_sources) = attacker.damage_sources {
            damage_sources
                .damage_sources
                .retain(|damage_source| damage_source.entity != revived.entity);
        }

        if let Some(mut npc_ai) = attacker.npc_ai {
            npc_ai
                .pending_damage
                .retain(|(attacker_entity, _)| *attacker_entity != revived.entity);
        }
    }
}

fn build_revived_spawn_character_message(
    revived: &ReviveEntityQueryItem,
    revived_entity: Entity,
    health_points: HealthPoints,
    position: &Position,
    clan_query: &Query<&Clan>,
) -> SpawnEntityCharacter {
    SpawnEntityCharacter {
        character_info: revived.character_info.clone(),
        spawn_command_state: SpawnCommandState::Stop,
        entity_id: revived.client_entity.id,
        equipment: revived.equipment.clone(),
        health: health_points,
        level: *revived.level,
        move_mode: MoveMode::Run,
        move_speed: *revived.move_speed,
        passive_attack_speed: revived.ability_values.passive_attack_speed,
        position: position.position,
        status_effects: StatusEffects::default().active.clone(),
        team: revived.team.clone(),
        personal_store_info: revived
            .personal_store
            .map(|personal_store| (personal_store.skin, personal_store.title.clone())),
        clan_membership: revived
            .clan_membership
            .and_then(|clan_membership| clan_membership.clan())
            .and_then(|clan_entity| clan_query.get(clan_entity).ok())
            .map(|clan| CharacterClanMembership {
                clan_unique_id: clan.unique_id,
                mark: clan.mark,
                level: clan.level,
                name: clan.name.clone(),
                position: clan
                    .find_online_member(revived_entity)
                    .map_or(ClanMemberPosition::Junior, |member| member.position()),
            }),
    }
}

fn revive_entity(
    commands: &mut Commands,
    client_entity_list: &mut ClientEntityList,
    entity: Entity,
    ability_values: &AbilityValues,
    client_entity: &ClientEntity,
    client_entity_sector: &ClientEntitySector,
    position: &Position,
    new_position: Position,
    game_client: Option<&GameClient>,
) {
    let status_effects = StatusEffects::default();

    if let Some(game_client) = game_client {
        game_client
            .server_message_tx
            .send(ServerMessage::UpdateStatusEffects {
                entity_id: client_entity.id,
                status_effects: status_effects.active.clone(),
                updated_values: Vec::new(),
            })
            .ok();
    }

    commands.entity(entity).remove::<Dead>().insert((
        HealthPoints::new((3 * ability_values.get_max_health()) / 10),
        ManaPoints::new((3 * ability_values.get_max_mana()) / 10),
        status_effects,
        StatusEffectsRegen::default(),
        MoveMode::Run,
        Command::with_stop(),
        NextCommand::default(),
        DamageSources::default_character(),
        PassiveRecoveryTime::default(),
    ));

    client_entity_teleport_zone(
        commands,
        client_entity_list,
        entity,
        client_entity,
        client_entity_sector,
        position,
        new_position,
        game_client,
    );
}

fn revive_entity_same_zone(
    commands: &mut Commands,
    client_entity_list: &mut ClientEntityList,
    revived: &mut ReviveEntityQueryItem,
    new_position: Position,
    query_observers: &mut Query<ReviveObserverQuery, Without<Dead>>,
    query_attackers: &mut Query<ReviveAggroQuery, (With<NpcAi>, Without<Dead>)>,
    clan_query: &Query<&Clan>,
    server_messages: &mut ServerMessages,
) {
    let status_effects = StatusEffects::default();
    let health_points = HealthPoints::new((3 * revived.ability_values.get_max_health()) / 10);
    let mana_points = ManaPoints::new((3 * revived.ability_values.get_max_mana()) / 10);

    if let Some(game_client) = revived.game_client {
        game_client
            .server_message_tx
            .send(ServerMessage::UpdateStatusEffects {
                entity_id: revived.client_entity.id,
                status_effects: status_effects.active.clone(),
                updated_values: Vec::new(),
            })
            .ok();
        game_client
            .server_message_tx
            .send(ServerMessage::UpdateAbilityValueSet {
                ability_type: AbilityType::Health,
                value: health_points.hp,
            })
            .ok();
        game_client
            .server_message_tx
            .send(ServerMessage::UpdateAbilityValueSet {
                ability_type: AbilityType::Mana,
                value: mana_points.mp,
            })
            .ok();
        game_client
            .server_message_tx
            .send(ServerMessage::AdjustPosition {
                entity_id: revived.client_entity.id,
                position: new_position.position,
            })
            .ok();
    }

    commands.entity(revived.entity).remove::<Dead>().insert((
        health_points,
        mana_points,
        status_effects,
        StatusEffectsRegen::default(),
        MoveMode::Run,
        Command::with_stop(),
        NextCommand::default(),
        DamageSources::default_character(),
        PassiveRecoveryTime::default(),
        new_position.clone(),
    ));

    if let Some(zone) = client_entity_list.get_zone_mut(new_position.zone_id) {
        zone.update_position(
            revived.entity,
            revived.client_entity,
            &mut revived.client_entity_sector,
            new_position.position,
        );
    }

    clear_same_zone_aggro_to_revived(revived, query_attackers, server_messages);

    let Some(zone) = client_entity_list.get_zone(new_position.zone_id) else {
        return;
    };

    let spawn_message = build_revived_spawn_character_message(
        revived,
        revived.entity,
        health_points,
        &new_position,
        clan_query,
    );

    for mut observer in query_observers.iter_mut() {
        if observer.entity == revived.entity || observer.position.zone_id != new_position.zone_id {
            continue;
        }

        let had_visible = observer
            .client_entity_visibility
            .entities
            .get(revived.client_entity.id.0)
            .map_or(false, |visible| *visible);
        if !had_visible {
            continue;
        }

        observer
            .game_client
            .server_message_tx
            .send(ServerMessage::RemoveEntities {
                entity_ids: vec![revived.client_entity.id],
            })
            .ok();

        let still_visible = zone
            .get_sector_visible_entities(observer.client_entity_sector.sector)
            .get(revived.client_entity.id.0)
            .map_or(false, |visible| *visible);

        if still_visible {
            observer
                .game_client
                .server_message_tx
                .send(ServerMessage::SpawnEntityCharacter {
                    data: Box::new(spawn_message.clone()),
                })
                .ok();
        } else {
            observer
                .client_entity_visibility
                .entities
                .set(revived.client_entity.id.0, false);
        }
    }
}

pub fn revive_event_system(
    mut commands: Commands,
    mut events: EventReader<ReviveEvent>,
    mut query: Query<ReviveEntityQuery, With<Dead>>,
    mut query_observers: Query<ReviveObserverQuery, Without<Dead>>,
    mut query_attackers: Query<ReviveAggroQuery, (With<NpcAi>, Without<Dead>)>,
    clan_query: Query<&Clan>,
    game_data: Res<GameData>,
    mut client_entity_list: ResMut<ClientEntityList>,
    mut server_messages: ResMut<ServerMessages>,
) {
    let mut rng = rand::thread_rng();

    for event in events.iter() {
        let Ok(mut entity) = query.get_mut(event.entity) else {
            continue;
        };

        let mut new_position = match event.position {
            RevivePosition::CurrentZone => {
                let revive_position =
                    if let Some(zone_data) = game_data.zones.get_zone(entity.position.zone_id) {
                        if let Some(revive_position) =
                            zone_data.get_closest_revive_position(entity.position.position)
                        {
                            revive_position
                        } else {
                            zone_data.start_position
                        }
                    } else {
                        entity.position.position
                    };

                Position::new(revive_position, entity.position.zone_id)
            }
            RevivePosition::SaveZone => Position::new(
                entity.character_info.revive_position,
                entity.character_info.revive_zone_id,
            ),
        };

        // Randomise respawn position
        new_position.position = Vec3::new(
            new_position.position.x + rng.gen_range(-REVIVE_SPAWN_RADIUS..=REVIVE_SPAWN_RADIUS),
            new_position.position.y + rng.gen_range(-REVIVE_SPAWN_RADIUS..=REVIVE_SPAWN_RADIUS),
            new_position.position.z,
        );

        if new_position.zone_id == entity.position.zone_id && entity.game_client.is_some() {
            revive_entity_same_zone(
                &mut commands,
                &mut client_entity_list,
                &mut entity,
                new_position,
                &mut query_observers,
                &mut query_attackers,
                &clan_query,
                &mut server_messages,
            );
        } else {
            revive_entity(
                &mut commands,
                &mut client_entity_list,
                entity.entity,
                entity.ability_values,
                entity.client_entity,
                &entity.client_entity_sector,
                entity.position,
                new_position,
                entity.game_client,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use bevy::{
        app::{App, Update},
        ecs::{
            query::{With, Without},
            system::{Commands, Query, Res, ResMut, Resource},
        },
        math::{UVec2, Vec2, Vec3},
        prelude::Entity,
    };
    use crossbeam_channel::unbounded as crossbeam_unbounded;
    use rose_data::{StatusEffectId, StatusEffectType, ZoneData, ZoneId};
    use rose_game_common::{
        components::{
            AbilityValues, AbilityValuesAdjust, ActiveStatusEffect, ActiveStatusEffectRegen,
            CharacterGender, CharacterInfo, Equipment, HealthPoints, Level, ManaPoints, MoveSpeed,
            Npc, StatusEffects, StatusEffectsRegen, Team,
        },
        data::Damage,
        messages::{
            server::{ServerMessage, SpawnCommandState},
            ClientEntityId,
        },
    };
    use tokio::sync::mpsc::{error::TryRecvError, unbounded_channel, UnboundedReceiver};

    use super::{
        revive_entity, revive_entity_same_zone, ReviveAggroQuery, ReviveEntityQuery,
        ReviveObserverQuery,
    };
    use crate::game::{
        components::{
            Clan, ClientEntity, ClientEntitySector, ClientEntityType, ClientEntityVisibility,
            Command, CommandData, DamageSources, Dead, GameClient, NextCommand, NpcAi, Position,
        },
        resources::{ClientEntityList, ClientEntityZone, ServerMessages},
    };

    #[derive(Resource, Clone, Copy)]
    struct TestEntity(Entity);

    #[derive(Resource, Clone)]
    struct TestRevivePosition(Position);

    fn test_ability_values() -> AbilityValues {
        AbilityValues {
            is_driving: false,
            damage_category: rose_game_common::components::DamageCategory::Character,
            level: 1,
            walk_speed: 200.0,
            run_speed: 425.0,
            vehicle_move_speed: 0.0,
            strength: 10,
            dexterity: 10,
            intelligence: 10,
            concentration: 10,
            charm: 10,
            sense: 10,
            max_health: 100,
            max_mana: 60,
            additional_health_recovery: 0,
            additional_mana_recovery: 0,
            attack_damage_type: rose_game_common::components::DamageType::Physical,
            attack_power: 10,
            attack_speed: 100,
            passive_attack_speed: 0,
            attack_range: 100,
            hit: 10,
            defence: 10,
            resistance: 10,
            critical: 10,
            avoid: 10,
            vehicle_attack_power: 0,
            vehicle_attack_range: 0,
            vehicle_attack_speed: 0,
            vehicle_hit: 0,
            vehicle_defence: 0,
            vehicle_critical: 0,
            vehicle_avoid: 0,
            max_damage_sources: 5,
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

    fn test_character_info(revive_zone_id: ZoneId, revive_position: Vec3) -> CharacterInfo {
        CharacterInfo {
            name: "ReviveTest".to_string(),
            gender: CharacterGender::Male,
            race: 0,
            birth_stone: 0,
            job: 0,
            face: 0,
            hair: 0,
            rank: 0,
            fame: 0,
            fame_b: 0,
            fame_g: 0,
            revive_zone_id,
            revive_position,
            unique_id: 1,
        }
    }

    fn drain_server_messages(
        server_message_rx: &mut UnboundedReceiver<ServerMessage>,
    ) -> Vec<ServerMessage> {
        let mut messages = Vec::new();
        loop {
            match server_message_rx.try_recv() {
                Ok(message) => messages.push(message),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        messages
    }

    fn test_zone_data(zone_id: ZoneId, revive_position: Vec3) -> ZoneData {
        ZoneData {
            id: zone_id,
            source_zone_path: std::path::PathBuf::new(),
            name: "Test Zone",
            description: "Test Zone",
            pvp_state: 0,
            join_trigger: None,
            kill_trigger: None,
            dead_trigger: None,
            sector_size: 5000,
            grid_per_patch: 16.0,
            grid_size: 250.0,
            event_objects: Vec::new(),
            monster_spawns: Vec::new(),
            npcs: Vec::new(),
            sectors_base_position: Vec2::ZERO,
            num_sectors_x: 2,
            num_sectors_y: 2,
            start_position: revive_position,
            revive_positions: vec![revive_position],
            event_positions: HashMap::new(),
            day_cycle: 1,
            morning_time: 0,
            day_time: 0,
            evening_time: 0,
            night_time: 0,
            skybox_id: None,
            party_xp_a: 0,
            party_xp_b: 0,
        }
    }

    #[test]
    fn revive_clears_status_effects_and_notifies_before_teleport() {
        fn system(
            mut commands: Commands,
            mut query: Query<ReviveEntityQuery, With<Dead>>,
            entity: Res<TestEntity>,
            revive_position: Res<TestRevivePosition>,
            mut client_entity_list: ResMut<ClientEntityList>,
        ) {
            let entity = query.get_mut(entity.0).expect("missing revive entity");
            revive_entity(
                &mut commands,
                &mut client_entity_list,
                entity.entity,
                entity.ability_values,
                entity.client_entity,
                &entity.client_entity_sector,
                entity.position,
                revive_position.0.clone(),
                entity.game_client,
            );
        }

        let mut app = App::new();
        app.insert_resource(ClientEntityList {
            zones: HashMap::new(),
        });
        app.insert_resource(ServerMessages::default());

        let source_zone = ZoneId::new(1).unwrap();
        let target_zone = ZoneId::new(2).unwrap();
        let target_position = Vec3::new(900.0, 1100.0, 0.0);
        let (client_message_tx, client_message_rx) = crossbeam_unbounded();
        drop(client_message_tx);
        let (server_message_tx, mut server_message_rx) = unbounded_channel();

        let mut status_effects = StatusEffects::default();
        status_effects.active[StatusEffectType::Poisoned] = Some(ActiveStatusEffect {
            id: StatusEffectId::new(7).unwrap(),
            value: 12,
        });
        status_effects.expire_times[StatusEffectType::Poisoned] =
            Some(std::time::Instant::now() + Duration::from_secs(10));

        let mut status_effects_regen = StatusEffectsRegen::default();
        status_effects_regen.regens[StatusEffectType::Poisoned] = Some(ActiveStatusEffectRegen {
            total_value: 12,
            value_per_second: 3,
            applied_value: 3,
            applied_duration: Duration::from_secs(1),
        });
        status_effects_regen.per_second_tick_counter = Duration::from_millis(500);

        let entity = app
            .world
            .spawn((
                Dead,
                test_ability_values(),
                ClientEntity::new(ClientEntityType::Character, ClientEntityId(42), source_zone),
                ClientEntitySector::new(UVec2::ZERO),
                test_character_info(target_zone, target_position),
                Equipment::default(),
                Level::new(12),
                MoveSpeed::new(425.0),
                Position::new(Vec3::new(100.0, 200.0, 0.0), source_zone),
                Team::default_character(),
                GameClient::new(client_message_rx, server_message_tx),
                HealthPoints::new(1),
                ManaPoints::new(1),
                status_effects,
                status_effects_regen,
            ))
            .id();

        app.insert_resource(TestEntity(entity));
        app.insert_resource(TestRevivePosition(Position::new(
            target_position,
            target_zone,
        )));
        app.add_systems(Update, system);

        app.update();

        let entity_ref = app.world.entity(entity);
        assert!(entity_ref.get::<Dead>().is_none());
        assert_eq!(entity_ref.get::<HealthPoints>().unwrap().hp, 30);
        assert_eq!(entity_ref.get::<ManaPoints>().unwrap().mp, 18);
        assert_eq!(entity_ref.get::<Position>().unwrap().zone_id, target_zone);
        assert_eq!(
            entity_ref.get::<Position>().unwrap().position,
            target_position
        );

        let status_effects = entity_ref.get::<StatusEffects>().unwrap();
        assert!(status_effects
            .active
            .iter()
            .all(|(_, effect)| effect.is_none()));
        assert!(status_effects
            .expire_times
            .iter()
            .all(|(_, expire_time)| expire_time.is_none()));

        let status_effects_regen = entity_ref.get::<StatusEffectsRegen>().unwrap();
        assert!(status_effects_regen
            .regens
            .iter()
            .all(|(_, regen)| regen.is_none()));
        assert_eq!(status_effects_regen.per_second_tick_counter, Duration::ZERO);

        let messages = drain_server_messages(&mut server_message_rx);
        assert_eq!(messages.len(), 2);

        match &messages[0] {
            ServerMessage::UpdateStatusEffects {
                entity_id,
                status_effects,
                updated_values,
            } => {
                assert_eq!(*entity_id, ClientEntityId(42));
                assert!(status_effects.iter().all(|(_, effect)| effect.is_none()));
                assert!(updated_values.is_empty());
            }
            other => panic!("expected UpdateStatusEffects, got {:?}", other),
        }

        match &messages[1] {
            ServerMessage::Teleport {
                entity_id,
                zone_id,
                x,
                y,
                ..
            } => {
                assert_eq!(*entity_id, ClientEntityId(42));
                assert_eq!(*zone_id, target_zone);
                assert_eq!(*x, target_position.x);
                assert_eq!(*y, target_position.y);
            }
            other => panic!("expected Teleport, got {:?}", other),
        }
    }

    #[test]
    fn same_zone_revive_uses_adjust_position_and_refreshes_observers() {
        fn system(
            mut commands: Commands,
            mut query: Query<ReviveEntityQuery, With<Dead>>,
            mut observers: Query<ReviveObserverQuery, Without<Dead>>,
            mut attackers: Query<ReviveAggroQuery, (With<NpcAi>, Without<Dead>)>,
            clan_query: Query<&Clan>,
            entity: Res<TestEntity>,
            revive_position: Res<TestRevivePosition>,
            mut client_entity_list: ResMut<ClientEntityList>,
            mut server_messages: ResMut<ServerMessages>,
        ) {
            let mut entity = query.get_mut(entity.0).expect("missing revive entity");
            revive_entity_same_zone(
                &mut commands,
                &mut client_entity_list,
                &mut entity,
                revive_position.0.clone(),
                &mut observers,
                &mut attackers,
                &clan_query,
                &mut server_messages,
            );
        }

        let mut app = App::new();

        let zone_id = ZoneId::new(1).unwrap();
        let start_position = Vec3::new(100.0, 200.0, 0.0);
        let revive_position = Vec3::new(600.0, 700.0, 0.0);
        let observer_position = Vec3::new(650.0, 750.0, 0.0);

        let mut zones = HashMap::new();
        zones.insert(
            zone_id,
            ClientEntityZone::new(&test_zone_data(zone_id, revive_position)),
        );
        app.insert_resource(ClientEntityList { zones });
        app.insert_resource(ServerMessages::default());

        let (revived_client_message_tx, revived_client_message_rx) = crossbeam_unbounded();
        drop(revived_client_message_tx);
        let (revived_server_message_tx, mut revived_server_message_rx) = unbounded_channel();

        let (observer_client_message_tx, observer_client_message_rx) = crossbeam_unbounded();
        drop(observer_client_message_tx);
        let (observer_server_message_tx, mut observer_server_message_rx) = unbounded_channel();

        let revived_entity = app
            .world
            .spawn((
                Dead,
                test_ability_values(),
                test_character_info(zone_id, revive_position),
                Equipment::default(),
                Level::new(12),
                MoveSpeed::new(425.0),
                Position::new(start_position, zone_id),
                Team::default_character(),
                ClientEntityVisibility::new(),
                GameClient::new(revived_client_message_rx, revived_server_message_tx),
                HealthPoints::new(1),
                ManaPoints::new(1),
                StatusEffects::default(),
                StatusEffectsRegen::default(),
            ))
            .id();

        let observer_entity = app
            .world
            .spawn((
                test_ability_values(),
                test_character_info(zone_id, observer_position),
                Equipment::default(),
                Level::new(18),
                MoveSpeed::new(425.0),
                Position::new(observer_position, zone_id),
                Team::default_character(),
                ClientEntityVisibility::new(),
                GameClient::new(observer_client_message_rx, observer_server_message_tx),
                HealthPoints::new(100),
                ManaPoints::new(50),
                StatusEffects::default(),
                StatusEffectsRegen::default(),
            ))
            .id();

        {
            let mut client_entity_list = app.world.resource_mut::<ClientEntityList>();
            let zone = client_entity_list
                .get_zone_mut(zone_id)
                .expect("missing test zone");

            let (revived_client_entity, revived_sector) = zone
                .join_zone(ClientEntityType::Character, revived_entity, start_position)
                .expect("failed to add revived entity");
            let (observer_client_entity, observer_sector) = zone
                .join_zone(
                    ClientEntityType::Character,
                    observer_entity,
                    observer_position,
                )
                .expect("failed to add observer entity");

            app.world
                .entity_mut(revived_entity)
                .insert((revived_client_entity.clone(), revived_sector));
            app.world
                .entity_mut(observer_entity)
                .insert((observer_client_entity, observer_sector));

            let mut observer_entity_mut = app.world.entity_mut(observer_entity);
            let mut observer_visibility = observer_entity_mut
                .get_mut::<ClientEntityVisibility>()
                .expect("missing observer visibility");
            observer_visibility
                .entities
                .set(revived_client_entity.id.0, true);
        }

        app.insert_resource(TestEntity(revived_entity));
        app.insert_resource(TestRevivePosition(Position::new(revive_position, zone_id)));
        app.add_systems(Update, system);

        app.update();

        let entity_ref = app.world.entity(revived_entity);
        assert!(entity_ref.get::<Dead>().is_none());
        assert_eq!(
            entity_ref.get::<Position>().unwrap().position,
            revive_position
        );
        assert_eq!(entity_ref.get::<HealthPoints>().unwrap().hp, 30);
        assert_eq!(entity_ref.get::<ManaPoints>().unwrap().mp, 18);

        let revived_messages = drain_server_messages(&mut revived_server_message_rx);
        assert_eq!(revived_messages.len(), 4);
        assert!(matches!(
            revived_messages[0],
            ServerMessage::UpdateStatusEffects { .. }
        ));
        assert!(matches!(
            revived_messages[1],
            ServerMessage::UpdateAbilityValueSet {
                ability_type: rose_data::AbilityType::Health,
                value: 30,
            }
        ));
        assert!(matches!(
            revived_messages[2],
            ServerMessage::UpdateAbilityValueSet {
                ability_type: rose_data::AbilityType::Mana,
                value: 18,
            }
        ));
        match &revived_messages[3] {
            ServerMessage::AdjustPosition {
                entity_id,
                position,
            } => {
                assert_eq!(*entity_id, entity_ref.get::<ClientEntity>().unwrap().id);
                assert_eq!(*position, revive_position);
            }
            other => panic!("expected AdjustPosition, got {:?}", other),
        }

        let observer_messages = drain_server_messages(&mut observer_server_message_rx);
        assert_eq!(observer_messages.len(), 2);
        match &observer_messages[0] {
            ServerMessage::RemoveEntities { entity_ids } => {
                assert_eq!(
                    entity_ids.as_slice(),
                    &[entity_ref.get::<ClientEntity>().unwrap().id]
                );
            }
            other => panic!("expected RemoveEntities, got {:?}", other),
        }

        match &observer_messages[1] {
            ServerMessage::SpawnEntityCharacter { data } => {
                assert_eq!(data.position, revive_position);
                assert_eq!(data.health.hp, 30);
                assert!(matches!(data.spawn_command_state, SpawnCommandState::Stop));
            }
            other => panic!("expected SpawnEntityCharacter, got {:?}", other),
        }

        assert!(app
            .world
            .resource::<ServerMessages>()
            .pending_entity_messages
            .is_empty());
    }

    #[test]
    fn same_zone_revive_clears_monster_aggro_and_pending_attack_state() {
        fn system(
            mut commands: Commands,
            mut query: Query<ReviveEntityQuery, With<Dead>>,
            mut observers: Query<ReviveObserverQuery, Without<Dead>>,
            mut attackers: Query<ReviveAggroQuery, (With<NpcAi>, Without<Dead>)>,
            clan_query: Query<&Clan>,
            entity: Res<TestEntity>,
            revive_position: Res<TestRevivePosition>,
            mut client_entity_list: ResMut<ClientEntityList>,
            mut server_messages: ResMut<ServerMessages>,
        ) {
            let mut entity = query.get_mut(entity.0).expect("missing revive entity");
            revive_entity_same_zone(
                &mut commands,
                &mut client_entity_list,
                &mut entity,
                revive_position.0.clone(),
                &mut observers,
                &mut attackers,
                &clan_query,
                &mut server_messages,
            );
        }

        let mut app = App::new();

        let zone_id = ZoneId::new(1).unwrap();
        let start_position = Vec3::new(100.0, 200.0, 0.0);
        let revive_position = Vec3::new(600.0, 700.0, 0.0);
        let monster_position = Vec3::new(500.0, 650.0, 0.0);

        let mut zones = HashMap::new();
        zones.insert(
            zone_id,
            ClientEntityZone::new(&test_zone_data(zone_id, revive_position)),
        );
        app.insert_resource(ClientEntityList { zones });
        app.insert_resource(ServerMessages::default());

        let (revived_client_message_tx, revived_client_message_rx) = crossbeam_unbounded();
        drop(revived_client_message_tx);
        let (revived_server_message_tx, mut revived_server_message_rx) = unbounded_channel();

        let revived_entity = app
            .world
            .spawn((
                Dead,
                test_ability_values(),
                test_character_info(zone_id, revive_position),
                Equipment::default(),
                Level::new(12),
                MoveSpeed::new(425.0),
                Position::new(start_position, zone_id),
                Team::default_character(),
                ClientEntityVisibility::new(),
                GameClient::new(revived_client_message_rx, revived_server_message_tx),
                HealthPoints::new(1),
                ManaPoints::new(1),
                StatusEffects::default(),
                StatusEffectsRegen::default(),
            ))
            .id();

        let monster_entity = app
            .world
            .spawn((
                test_ability_values(),
                Npc::new(rose_data::NpcId::new(1).unwrap(), 0),
                NpcAi::new(1),
                Position::new(monster_position, zone_id),
                ClientEntity::new(ClientEntityType::Monster, ClientEntityId(77), zone_id),
                Command::with_attack(revived_entity, Duration::from_secs(1)),
                NextCommand::with_attack(revived_entity),
                DamageSources {
                    max_damage_sources: 5,
                    damage_sources: vec![crate::game::components::DamageSource {
                        entity: revived_entity,
                        total_damage: 50,
                        first_damage_time: std::time::Instant::now(),
                        last_damage_time: std::time::Instant::now(),
                    }],
                },
                Team::default_npc(),
                HealthPoints::new(100),
            ))
            .id();

        {
            let mut monster_entity_mut = app.world.entity_mut(monster_entity);
            let mut npc_ai = monster_entity_mut.get_mut::<NpcAi>().unwrap();
            npc_ai.pending_damage.push((
                revived_entity,
                Damage {
                    amount: 15,
                    is_critical: false,
                    apply_hit_stun: false,
                },
            ));
        }

        {
            let mut client_entity_list = app.world.resource_mut::<ClientEntityList>();
            let zone = client_entity_list
                .get_zone_mut(zone_id)
                .expect("missing test zone");

            let (revived_client_entity, revived_sector) = zone
                .join_zone(ClientEntityType::Character, revived_entity, start_position)
                .expect("failed to add revived entity");
            let (monster_client_entity, monster_sector) = zone
                .join_zone(ClientEntityType::Monster, monster_entity, monster_position)
                .expect("failed to add monster entity");

            app.world
                .entity_mut(revived_entity)
                .insert((revived_client_entity, revived_sector));
            app.world
                .entity_mut(monster_entity)
                .insert((monster_client_entity, monster_sector));
        }

        app.insert_resource(TestEntity(revived_entity));
        app.insert_resource(TestRevivePosition(Position::new(revive_position, zone_id)));
        app.add_systems(Update, system);

        app.update();

        let monster_ref = app.world.entity(monster_entity);
        assert!(matches!(
            monster_ref.get::<Command>().unwrap().command,
            CommandData::Stop { .. }
        ));
        assert!(monster_ref.get::<NextCommand>().unwrap().command.is_none());
        assert!(monster_ref
            .get::<DamageSources>()
            .unwrap()
            .damage_sources
            .is_empty());
        assert!(monster_ref
            .get::<NpcAi>()
            .unwrap()
            .pending_damage
            .is_empty());

        let pending_entity_messages = &app
            .world
            .resource::<ServerMessages>()
            .pending_entity_messages;
        assert_eq!(pending_entity_messages.len(), 1);
        match &pending_entity_messages[0].message {
            ServerMessage::StopMoveEntity { entity_id, x, y, z } => {
                assert_eq!(*entity_id, monster_ref.get::<ClientEntity>().unwrap().id);
                assert_eq!(*x, monster_position.x);
                assert_eq!(*y, monster_position.y);
                assert_eq!(*z, monster_position.z as u16);
            }
            other => panic!("expected StopMoveEntity, got {:?}", other),
        }

        let revived_messages = drain_server_messages(&mut revived_server_message_rx);
        assert_eq!(revived_messages.len(), 4);
    }
}
