use bevy::{
    ecs::{prelude::Query, world::Ref},
    prelude::DetectChanges,
};

use crate::game::{
    components::{AbilityValues, ClientEntity, MoveSpeed},
    messages::server::ServerMessage,
    resources::ServerMessages,
};

pub fn move_speed_sync_system(
    query: Query<(&ClientEntity, &AbilityValues, Ref<MoveSpeed>)>,
    mut server_messages: bevy::ecs::prelude::ResMut<ServerMessages>,
) {
    for (client_entity, ability_values, move_speed) in query.iter() {
        if !move_speed.is_changed() || move_speed.is_added() {
            continue;
        }

        server_messages.send_entity_message(
            client_entity,
            ServerMessage::UpdateSpeed {
                entity_id: client_entity.id,
                run_speed: move_speed.speed as i32,
                passive_attack_speed: ability_values.get_passive_attack_speed(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use bevy::{app::Update, prelude::App};
    use rose_data::ZoneId;

    use super::move_speed_sync_system;
    use crate::game::{
        components::{
            AbilityValues, ClientEntity, ClientEntityId, ClientEntityType, DamageCategory,
            DamageType, MoveSpeed,
        },
        messages::server::ServerMessage,
        resources::ServerMessages,
    };
    use rose_game_common::components::AbilityValuesAdjust;

    fn test_ability_values() -> AbilityValues {
        AbilityValues {
            is_driving: false,
            damage_category: DamageCategory::Character,
            level: 1,
            walk_speed: 200.0,
            run_speed: 400.0,
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
            attack_speed: 120,
            passive_attack_speed: 15,
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

    #[test]
    fn changed_move_speed_sends_update_speed() {
        let mut app = App::new();
        app.insert_resource(ServerMessages::default());
        let entity = app
            .world
            .spawn((
                ClientEntity::new(
                    ClientEntityType::Character,
                    ClientEntityId(7),
                    ZoneId::new(1).unwrap(),
                ),
                test_ability_values(),
                MoveSpeed::new(400.0),
            ))
            .id();
        app.add_systems(Update, move_speed_sync_system);

        app.update();
        app.world
            .resource_mut::<ServerMessages>()
            .pending_entity_messages
            .clear();

        app.world.entity_mut(entity).insert(MoveSpeed::new(275.0));
        app.update();

        let messages = &app
            .world
            .resource::<ServerMessages>()
            .pending_entity_messages;
        assert_eq!(messages.len(), 1);
        match &messages[0].message {
            ServerMessage::UpdateSpeed {
                entity_id,
                run_speed,
                passive_attack_speed,
            } => {
                assert_eq!(*entity_id, ClientEntityId(7));
                assert_eq!(*run_speed, 275);
                assert_eq!(*passive_attack_speed, 15);
            }
            other => panic!("expected UpdateSpeed, got {:?}", other),
        }
    }

    #[test]
    fn added_move_speed_does_not_send_update_speed() {
        let mut app = App::new();
        app.insert_resource(ServerMessages::default());
        app.world.spawn((
            ClientEntity::new(
                ClientEntityType::Character,
                ClientEntityId(8),
                ZoneId::new(1).unwrap(),
            ),
            test_ability_values(),
            MoveSpeed::new(400.0),
        ));
        app.add_systems(Update, move_speed_sync_system);

        app.update();

        assert!(app
            .world
            .resource::<ServerMessages>()
            .pending_entity_messages
            .is_empty());
    }
}
