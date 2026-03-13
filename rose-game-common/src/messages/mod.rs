use bevy::reflect::Reflect;
use serde::{Deserialize, Serialize};

use crate::components::CharacterUniqueId;

pub mod client;
pub mod server;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
pub struct ClientEntityId(pub usize);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
pub struct Friend {
    pub character_id: CharacterUniqueId,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
pub enum FriendStatus {
    Online,
    Offline,
    Refused,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Reflect)]
pub struct FriendInfo {
    pub character_id: CharacterUniqueId,
    pub name: String,
    pub status: FriendStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartyRejectInviteReason {
    Busy,
    Reject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartyXpSharing {
    EqualShare,
    DistributedByLevel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartyItemSharing {
    EqualLootDistribution,
    AcquisitionOrder,
}
