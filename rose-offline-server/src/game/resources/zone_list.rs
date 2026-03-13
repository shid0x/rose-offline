use bevy::{ecs::prelude::Entity, prelude::Resource};
use std::collections::HashMap;

use rose_data::{NpcId, ZoneId};

#[derive(Hash, PartialEq, Eq)]
struct EventObjectKey {
    event_id: u16,
    map_chunk_x: i32,
    map_chunk_y: i32,
}

struct ZoneData {
    monster_spawns_enabled: bool,
    pvp_enabled: bool,
    event_objects: HashMap<EventObjectKey, Entity>,
}

#[derive(Resource)]
pub struct ZoneList {
    zones: HashMap<ZoneId, ZoneData>,
    npcs: HashMap<NpcId, Entity>,
}

impl ZoneList {
    pub fn new() -> Self {
        Self {
            zones: Default::default(),
            npcs: Default::default(),
        }
    }

    pub fn add_zone(&mut self, zone_id: ZoneId, pvp_enabled: bool) {
        self.zones.insert(
            zone_id,
            ZoneData {
                monster_spawns_enabled: true,
                pvp_enabled,
                event_objects: Default::default(),
            },
        );
    }

    pub fn get_monster_spawns_enabled(&self, zone_id: ZoneId) -> bool {
        self.zones
            .get(&zone_id)
            .map(|zone| zone.monster_spawns_enabled)
            .unwrap_or(false)
    }

    pub fn set_monster_spawns_enabled(&mut self, zone_id: ZoneId, enabled: bool) -> bool {
        if let Some(zone) = self.zones.get_mut(&zone_id) {
            zone.monster_spawns_enabled = enabled;
            true
        } else {
            false
        }
    }

    pub fn get_pvp_enabled(&self, zone_id: ZoneId) -> bool {
        self.zones
            .get(&zone_id)
            .map(|zone| zone.pvp_enabled)
            .unwrap_or(false)
    }

    pub fn set_pvp_enabled(&mut self, zone_id: ZoneId, enabled: bool) -> bool {
        if let Some(zone) = self.zones.get_mut(&zone_id) {
            zone.pvp_enabled = enabled;
            true
        } else {
            false
        }
    }

    pub fn add_event_object(
        &mut self,
        zone_id: ZoneId,
        event_id: u16,
        map_chunk_x: i32,
        map_chunk_y: i32,
        entity: Entity,
    ) {
        if let Some(zone) = self.zones.get_mut(&zone_id) {
            zone.event_objects.insert(
                EventObjectKey {
                    event_id,
                    map_chunk_x,
                    map_chunk_y,
                },
                entity,
            );
        }
    }

    pub fn find_event_object(
        &self,
        zone_id: ZoneId,
        event_id: u16,
        map_chunk_x: i32,
        map_chunk_y: i32,
    ) -> Option<Entity> {
        self.zones.get(&zone_id).and_then(|zone| {
            zone.event_objects
                .get(&EventObjectKey {
                    event_id,
                    map_chunk_x,
                    map_chunk_y,
                })
                .cloned()
        })
    }

    pub fn add_npc(&mut self, npc_id: NpcId, entity: Entity) {
        self.npcs.insert(npc_id, entity);
    }

    pub fn find_npc(&self, npc_id: NpcId) -> Option<Entity> {
        self.npcs.get(&npc_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::ZoneList;
    use rose_data::ZoneId;

    #[test]
    fn zone_pvp_state_defaults_to_false_for_missing_zone() {
        let zone_list = ZoneList::new();
        let zone_id = ZoneId::new(1).unwrap();
        assert!(!zone_list.get_pvp_enabled(zone_id));
    }

    #[test]
    fn zone_pvp_state_can_be_toggled() {
        let mut zone_list = ZoneList::new();
        let zone_id = ZoneId::new(1).unwrap();

        zone_list.add_zone(zone_id, false);
        assert!(!zone_list.get_pvp_enabled(zone_id));

        assert!(zone_list.set_pvp_enabled(zone_id, true));
        assert!(zone_list.get_pvp_enabled(zone_id));

        assert!(zone_list.set_pvp_enabled(zone_id, false));
        assert!(!zone_list.get_pvp_enabled(zone_id));
    }
}
