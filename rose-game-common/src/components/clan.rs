use std::num::{NonZeroU16, NonZeroU32};

use bevy::prelude::{Deref, DerefMut};
use serde::{Deserialize, Serialize};

pub const MAX_CLAN_LEVEL: u32 = 7;

const DEFAULT_CLAN_UPGRADE_POINTS: [u64; (MAX_CLAN_LEVEL as usize) + 1] =
    [0, 0, 1_000, 2_500, 5_000, 10_000, 20_000, 35_000];

#[derive(Deref, DerefMut, Copy, Clone, Debug, Serialize, Deserialize)]
pub struct ClanUniqueId(pub NonZeroU32);

impl ClanUniqueId {
    pub fn new(n: u32) -> Option<ClanUniqueId> {
        NonZeroU32::new(n).map(ClanUniqueId)
    }
}

#[derive(Deref, DerefMut, Copy, Clone, Debug, Serialize, Deserialize)]
pub struct ClanLevel(pub NonZeroU32);

impl ClanLevel {
    pub fn new(n: u32) -> Option<ClanLevel> {
        NonZeroU32::new(n).map(ClanLevel)
    }
}

#[derive(Deref, DerefMut, Copy, Clone, Debug, Serialize, Deserialize)]
pub struct ClanPoints(pub u64);

pub fn get_default_clan_upgrade_points(next_level: u32) -> Option<ClanPoints> {
    if !(2..=MAX_CLAN_LEVEL).contains(&next_level) {
        return None;
    }

    Some(ClanPoints(DEFAULT_CLAN_UPGRADE_POINTS[next_level as usize]))
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub enum ClanMark {
    Premade {
        background: NonZeroU16,
        foreground: NonZeroU16,
    },
    Custom {
        crc16: u16,
    },
}
