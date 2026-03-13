use arrayvec::ArrayVec;
use bevy::ecs::prelude::{Component, Entity};
use enum_map::{enum_map, EnumMap};

use rose_game_common::{
    components::InventoryPageType,
    messages::{PartyItemSharing, PartyXpSharing},
};

use crate::game::components::CharacterUniqueId;

pub const MAX_PARTY_LEVEL: i32 = 50;

/// XP threshold reduced by 5x from original for faster party leveling in offline play.
pub fn party_level_up_need_xp(level: i32) -> i32 {
    (level + 7) * (level + 10) + 40
}

pub fn party_xp_gain(diff_level: i32, zone_party_xp_a: i64, zone_party_xp_b: i64) -> i64 {
    if diff_level >= 0 {
        (diff_level as i64 + zone_party_xp_a)
            * (diff_level as i64 + zone_party_xp_a + 1)
            * zone_party_xp_b
            / 10
            + 2
    } else {
        2
    }
}

pub fn apply_party_xp_gain(party_level: i32, party_experience: i32, gain: i64) -> (i32, i32, bool) {
    let mut new_level = party_level;
    let mut new_xp = party_experience as i64 + gain.max(0);
    let mut is_level_up = false;

    if new_level < MAX_PARTY_LEVEL {
        let need_xp = party_level_up_need_xp(new_level) as i64;
        if new_xp >= need_xp {
            new_xp -= need_xp;
            new_level += 1;
            is_level_up = true;
        }
    }

    (
        new_level,
        new_xp.clamp(0, i32::MAX as i64) as i32,
        is_level_up,
    )
}

#[derive(Clone)]
pub enum PartyMember {
    Online(Entity),
    Offline(CharacterUniqueId, String),
}

impl PartyMember {
    pub fn get_entity(&self) -> Option<Entity> {
        match self {
            PartyMember::Online(entity) => Some(*entity),
            PartyMember::Offline(_, _) => None,
        }
    }
}

#[derive(Component)]
pub struct Party {
    pub owner: Entity,
    pub members: ArrayVec<PartyMember, 7>,
    pub item_sharing: PartyItemSharing,
    pub xp_sharing: PartyXpSharing,
    pub average_member_level: i32,
    pub level: i32,
    pub experience: i32,
    pub acquire_item_order: EnumMap<InventoryPageType, usize>,
    pub acquire_money_order: usize,
}

impl Party {
    pub fn new(owner: Entity, party_members: &[PartyMember]) -> Self {
        let mut members = ArrayVec::new();

        for member in party_members {
            members.push(member.clone());
        }

        Self {
            owner,
            members,
            item_sharing: PartyItemSharing::EqualLootDistribution,
            xp_sharing: PartyXpSharing::EqualShare,
            average_member_level: 1,
            level: 1,
            experience: 0,
            acquire_item_order: enum_map! {
                _ => 0,
            },
            acquire_money_order: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_party_xp_gain, party_level_up_need_xp, party_xp_gain};

    #[test]
    fn party_xp_gain_is_positive_for_equal_level_kill() {
        assert!(party_xp_gain(0, 2, 2) > 0);
    }

    #[test]
    fn party_xp_level_up_rollover_preserves_remainder() {
        let need = party_level_up_need_xp(1) as i64;
        let (new_level, new_xp, is_level_up) = apply_party_xp_gain(1, 0, need + 5);

        assert_eq!(new_level, 2);
        assert_eq!(new_xp, 5);
        assert!(is_level_up);
    }

    #[test]
    fn qualifying_kills_do_not_reduce_party_xp_without_level_up() {
        let mut level = 20;
        let mut xp = 10;

        for _ in 0..500 {
            let previous_level = level;
            let previous_xp = xp;

            let (new_level, new_xp, _) = apply_party_xp_gain(level, xp, 2);

            if new_level == previous_level {
                assert!(new_xp >= previous_xp);
            }

            level = new_level;
            xp = new_xp;
        }
    }

    #[test]
    fn party_leveling_is_5x_faster_than_original() {
        // Original formula: (level+7)*(level+10)*5+200
        // New formula: (level+7)*(level+10)+40
        let original_need = (1 + 7) * (1 + 10) * 5 + 200; // 640
        let new_need = party_level_up_need_xp(1); // 128
        assert!(new_need < original_need / 4); // At least 4x faster
        assert!(new_need > original_need / 6); // But not more than 6x
    }
}
