use std::collections::HashMap;

use bevy::prelude::{Entity, Resource};
use rose_game_common::{components::CharacterUniqueId, messages::ClientEntityId};

#[derive(Clone, Copy)]
pub struct OnlineFriend {
    pub entity: Entity,
    pub client_entity_id: Option<ClientEntityId>,
}

#[derive(Default, Resource)]
pub struct OnlineFriends {
    by_id: HashMap<CharacterUniqueId, OnlineFriend>,
    by_name: HashMap<String, CharacterUniqueId>,
}

impl OnlineFriends {
    pub fn insert(
        &mut self,
        character_id: CharacterUniqueId,
        name: &str,
        entity: Entity,
        client_entity_id: Option<ClientEntityId>,
    ) {
        self.by_id.insert(
            character_id,
            OnlineFriend {
                entity,
                client_entity_id,
            },
        );
        self.by_name.insert(name.to_ascii_lowercase(), character_id);
    }

    pub fn update_client_entity_id(
        &mut self,
        character_id: CharacterUniqueId,
        client_entity_id: ClientEntityId,
    ) {
        if let Some(entry) = self.by_id.get_mut(&character_id) {
            entry.client_entity_id = Some(client_entity_id);
        }
    }

    pub fn remove(&mut self, character_id: CharacterUniqueId, name: &str) {
        self.by_id.remove(&character_id);
        self.by_name.remove(&name.to_ascii_lowercase());
    }

    pub fn get_by_id(&self, character_id: CharacterUniqueId) -> Option<OnlineFriend> {
        self.by_id.get(&character_id).copied()
    }

    pub fn get_by_name(&self, name: &str) -> Option<OnlineFriend> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .and_then(|character_id| self.by_id.get(character_id))
            .copied()
    }
}
