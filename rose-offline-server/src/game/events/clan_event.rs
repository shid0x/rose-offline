use bevy::prelude::{Entity, Event};

use rose_data::SkillId;
use rose_game_common::components::{ClanLevel, ClanMark, ClanPoints, Money};

use crate::game::components::Level;

#[derive(Event)]
pub enum ClanEvent {
    Create {
        creator: Entity,
        name: String,
        description: String,
        mark: ClanMark,
        skip_requirements: bool,
    },
    Invite {
        inviter_entity: Entity,
        name: String,
    },
    AcceptInvite {
        invited_entity: Entity,
        inviter_name: String,
    },
    RejectInvite {
        invited_entity: Entity,
        inviter_name: String,
    },
    Kick {
        kicker_entity: Entity,
        name: String,
    },
    Promote {
        changer_entity: Entity,
        name: String,
    },
    Demote {
        changer_entity: Entity,
        name: String,
    },
    Leave {
        leaver_entity: Entity,
    },
    Disband {
        entity: Entity,
    },
    MemberDisconnect {
        clan_entity: Entity,
        disconnect_entity: Entity,
        name: String,
        level: Level,
        job: u16,
    },
    GetMemberList {
        entity: Entity,
    },
    SetDescription {
        updater_entity: Entity,
        description: String,
    },
    AddLevel {
        clan_entity: Entity,
        level: i32,
    },
    SetLevel {
        clan_entity: Entity,
        level: ClanLevel,
    },
    AddMoney {
        clan_entity: Entity,
        money: i64,
    },
    SetMoney {
        clan_entity: Entity,
        money: Money,
    },
    AddPoints {
        clan_entity: Entity,
        points: i64,
    },
    SetPoints {
        clan_entity: Entity,
        points: ClanPoints,
    },
    AddSkill {
        clan_entity: Entity,
        skill_id: SkillId,
    },
    RemoveSkill {
        clan_entity: Entity,
        skill_id: SkillId,
    },
}
