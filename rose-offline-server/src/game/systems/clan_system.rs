use std::num::{NonZeroU32, NonZeroUsize};

use bevy::{
    ecs::query::WorldQuery,
    math::Vec3Swizzles,
    prelude::{Changed, Commands, Entity, EventReader, Query, Res, ResMut, With},
};

use rose_data::{ClanMemberPosition, QuestTriggerHash};
use rose_game_common::{
    components::{ClanLevel, ClanPoints, ClanUniqueId, MAX_CLAN_LEVEL},
    messages::{
        server::{
            ClanCreateError, ClanInviteResponse, ClanMemberInfo, ClanUpgradeResult, ServerMessage,
        },
        ClientEntityId,
    },
};

use crate::game::{
    components::{
        CharacterInfo, Clan, ClanMember, ClanMembership, ClientEntity, GameClient, Inventory,
        Level, Money, Npc, Position,
    },
    events::ClanEvent,
    resources::{GameConfig, ServerMessages},
    storage::clan::{ClanStorage, ClanStorageMember},
};

#[derive(WorldQuery)]
#[world_query(mutable)]
pub struct CreatorQuery<'w> {
    client_entity: &'w ClientEntity,
    character_info: &'w CharacterInfo,
    level: &'w Level,
    inventory: &'w mut Inventory,
    game_client: Option<&'w GameClient>,
    clan_membership: &'w ClanMembership,
}

#[derive(WorldQuery)]
pub struct MemberQuery<'w> {
    entity: Entity,
    character_info: &'w CharacterInfo,
    clan_membership: &'w ClanMembership,
    level: &'w Level,
    position: &'w Position,
    game_client: Option<&'w GameClient>,
    client_entity: Option<&'w ClientEntity>,
}

fn send_update_clan_info(clan: &Clan, query_member: &Query<MemberQuery>) {
    for clan_member in clan.members.iter() {
        let &ClanMember::Online {
            entity: clan_member_entity,
            ..
        } = clan_member
        else {
            continue;
        };

        if let Ok(online_member) = query_member.get(clan_member_entity) {
            if let Some(online_member_game_client) = online_member.game_client {
                online_member_game_client
                    .server_message_tx
                    .send(ServerMessage::ClanUpdateInfo {
                        id: clan.unique_id,
                        mark: clan.mark,
                        level: clan.level,
                        points: clan.points,
                        money: clan.money,
                        description: clan.description.clone(),
                        skills: clan.skills.clone(),
                    })
                    .ok();
            }
        }
    }
}

fn send_clan_upgrade_result(member: &MemberQueryItem, result: ClanUpgradeResult) {
    if let Some(game_client) = member.game_client {
        game_client
            .server_message_tx
            .send(ServerMessage::ClanUpgradeResult { result })
            .ok();
    }
}

fn send_character_update_clan_for_online_members(
    clan: &Clan,
    query_member: &Query<MemberQuery>,
    server_messages: &mut ServerMessages,
) {
    for clan_member in clan.members.iter() {
        let &ClanMember::Online {
            entity: clan_member_entity,
            position,
            ..
        } = clan_member
        else {
            continue;
        };

        let Ok(online_member) = query_member.get(clan_member_entity) else {
            continue;
        };
        let (Some(game_client), Some(client_entity)) =
            (online_member.game_client, online_member.client_entity)
        else {
            continue;
        };

        let update_message = ServerMessage::CharacterUpdateClan {
            client_entity_id: client_entity.id,
            id: clan.unique_id,
            name: clan.name.clone(),
            mark: clan.mark,
            level: clan.level,
            position,
        };

        game_client
            .server_message_tx
            .send(update_message.clone())
            .ok();
        server_messages.send_entity_message(client_entity, update_message);
    }
}

fn find_npc_entity(
    query_npc: &Query<(Entity, &ClientEntity, &Position), With<Npc>>,
    npc_entity_id: ClientEntityId,
) -> Option<(Entity, ClientEntityId, Position)> {
    query_npc
        .iter()
        .find_map(|(entity, client_entity, position)| {
            (client_entity.id == npc_entity_id).then_some((
                entity,
                client_entity.id,
                position.clone(),
            ))
        })
}

fn save_clan(clan: &Clan, query_member: &Query<MemberQuery>) {
    let mut members = Vec::new();
    for member in clan.members.iter() {
        match member {
            ClanMember::Online {
                entity,
                position,
                contribution,
            } => {
                if let Ok(online_member) = query_member.get(*entity) {
                    members.push(ClanStorageMember {
                        name: online_member.character_info.name.clone(),
                        position: *position,
                        contribution: *contribution,
                    });
                }
            }
            ClanMember::Offline {
                name,
                position,
                contribution,
                ..
            } => {
                members.push(ClanStorageMember {
                    name: name.clone(),
                    position: *position,
                    contribution: *contribution,
                });
            }
        }
    }

    let clan_storage = ClanStorage {
        name: clan.name.clone(),
        description: clan.description.clone(),
        mark: clan.mark,
        money: clan.money,
        points: clan.points,
        level: clan.level,
        members,
        skills: clan.skills.clone(),
    };
    if let Err(error) = clan_storage.save() {
        log::error!("Failed to save clan {}: {:?}", clan.name, error);
    }
}

/// Max member count per clan level: {15, 15, 20, 25, 30, 36, 43, 50}
fn clan_max_members(level: ClanLevel) -> usize {
    match level.get() {
        0 | 1 => 15,
        2 => 20,
        3 => 25,
        4 => 30,
        5 => 36,
        6 => 43,
        _ => 50,
    }
}

fn clan_member_position_rank(position: ClanMemberPosition) -> u8 {
    match position {
        ClanMemberPosition::Penalty => 0,
        ClanMemberPosition::Junior => 1,
        ClanMemberPosition::Senior => 2,
        ClanMemberPosition::Veteran => 3,
        ClanMemberPosition::Commander => 4,
        ClanMemberPosition::DeputyMaster => 5,
        ClanMemberPosition::Master => 6,
    }
}

fn clan_member_position_from_rank(rank: u8) -> Option<ClanMemberPosition> {
    match rank {
        0 => Some(ClanMemberPosition::Penalty),
        1 => Some(ClanMemberPosition::Junior),
        2 => Some(ClanMemberPosition::Senior),
        3 => Some(ClanMemberPosition::Veteran),
        4 => Some(ClanMemberPosition::Commander),
        5 => Some(ClanMemberPosition::DeputyMaster),
        6 => Some(ClanMemberPosition::Master),
        _ => None,
    }
}

fn can_change_member_rank(
    changer_position: ClanMemberPosition,
    target_position: ClanMemberPosition,
) -> bool {
    matches!(
        changer_position,
        ClanMemberPosition::Master | ClanMemberPosition::DeputyMaster
    ) && clan_member_position_rank(changer_position) > clan_member_position_rank(target_position)
}

fn get_promoted_position(
    changer_position: ClanMemberPosition,
    target_position: ClanMemberPosition,
) -> Option<ClanMemberPosition> {
    if !can_change_member_rank(changer_position, target_position) {
        return None;
    }

    let changer_rank = clan_member_position_rank(changer_position);
    let target_rank = clan_member_position_rank(target_position);
    let promoted_rank = target_rank.checked_add(1)?;
    if promoted_rank >= changer_rank {
        return None;
    }

    clan_member_position_from_rank(promoted_rank)
}

fn get_demoted_position(
    changer_position: ClanMemberPosition,
    target_position: ClanMemberPosition,
) -> Option<ClanMemberPosition> {
    if !can_change_member_rank(changer_position, target_position) {
        return None;
    }

    let demoted_rank = clan_member_position_rank(target_position).checked_sub(1)?;
    clan_member_position_from_rank(demoted_rank)
}

fn build_clan_member_list(clan: &Clan, query_member: &Query<MemberQuery>) -> Vec<ClanMemberInfo> {
    let mut members = Vec::new();

    for member in clan.members.iter() {
        match *member {
            ClanMember::Online {
                entity: member_entity,
                position,
                contribution,
            } => {
                if let Ok(member) = query_member.get(member_entity) {
                    members.push(ClanMemberInfo {
                        name: member.character_info.name.clone(),
                        position,
                        contribution,
                        channel_id: NonZeroUsize::new(1),
                        level: *member.level,
                        job: member.character_info.job,
                    });
                }
            }
            ClanMember::Offline {
                ref name,
                position,
                contribution,
                level,
                job,
            } => {
                members.push(ClanMemberInfo {
                    name: name.clone(),
                    position,
                    contribution,
                    channel_id: None,
                    level,
                    job,
                });
            }
        }
    }

    members
}

fn send_clan_member_list_to_online_members(clan: &Clan, query_member: &Query<MemberQuery>) {
    let members = build_clan_member_list(clan, query_member);

    for clan_member in clan.members.iter() {
        let &ClanMember::Online {
            entity: member_entity,
            ..
        } = clan_member
        else {
            continue;
        };

        if let Ok(online_member) = query_member.get(member_entity) {
            if let Some(game_client) = online_member.game_client {
                game_client
                    .server_message_tx
                    .send(ServerMessage::ClanMemberList {
                        members: members.clone(),
                    })
                    .ok();
            }
        }
    }
}

pub fn clan_system(
    mut commands: Commands,
    mut clan_events: EventReader<ClanEvent>,
    query_member_connected: Query<MemberQuery, Changed<ClanMembership>>,
    query_member: Query<MemberQuery>,
    query_npc: Query<(Entity, &ClientEntity, &Position), With<Npc>>,
    mut query_creator: Query<CreatorQuery>,
    mut query_clans: Query<(Entity, &mut Clan)>,
    game_config: Res<GameConfig>,
    mut server_messages: ResMut<ServerMessages>,
) {
    for event in clan_events.iter() {
        match event {
            ClanEvent::Create {
                creator: creator_entity,
                name,
                description,
                mark,
                skip_requirements,
            } => {
                let Ok(mut creator) = query_creator.get_mut(*creator_entity) else {
                    log::error!(
                        "Clan create: could not find creator entity {:?}",
                        creator_entity
                    );
                    continue;
                };

                // Cannot create a clan if already in one
                if creator.clan_membership.is_some() {
                    log::warn!("Clan create: creator already in a clan");
                    if let Some(game_client) = creator.game_client {
                        game_client
                            .server_message_tx
                            .send(ServerMessage::ClanCreateError {
                                error: ClanCreateError::Failed,
                            })
                            .ok();
                    }
                    continue;
                }

                if !skip_requirements && creator.level.level < 30 {
                    log::warn!("Clan create: creator level {} < 30", creator.level.level);
                    if let Some(game_client) = creator.game_client {
                        game_client
                            .server_message_tx
                            .send(ServerMessage::ClanCreateError {
                                error: ClanCreateError::UnmetCondition,
                            })
                            .ok();
                    }
                    continue;
                }

                if ClanStorage::exists(name) {
                    log::warn!("Clan create: clan name '{}' already exists", name);
                    if let Some(game_client) = creator.game_client {
                        game_client
                            .server_message_tx
                            .send(ServerMessage::ClanCreateError {
                                error: ClanCreateError::NameExists,
                            })
                            .ok();
                    }
                    continue;
                }

                let money = if !skip_requirements {
                    let Ok(money) = creator.inventory.try_take_money(Money(1000000)) else {
                        log::warn!("Clan create: creator does not have enough money");
                        if let Some(game_client) = creator.game_client {
                            game_client
                                .server_message_tx
                                .send(ServerMessage::ClanCreateError {
                                    error: ClanCreateError::UnmetCondition,
                                })
                                .ok();
                        }
                        continue;
                    };
                    money
                } else {
                    Money(0)
                };

                let mut clan_storage = ClanStorage::new(name.clone(), description.clone(), *mark);
                clan_storage.members.push(ClanStorageMember::new(
                    creator.character_info.name.clone(),
                    ClanMemberPosition::Master,
                ));
                if let Err(err) = clan_storage.try_create() {
                    log::error!("Clan create: try_create failed: {:?}", err);
                    if let Some(game_client) = creator.game_client {
                        game_client
                            .server_message_tx
                            .send(ServerMessage::ClanCreateError {
                                error: ClanCreateError::Failed,
                            })
                            .ok();
                    }

                    creator.inventory.try_add_money(money).ok();
                    continue;
                }
                log::info!(
                    "Clan '{}' created successfully by {}",
                    name,
                    creator.character_info.name
                );

                // Create clan entity
                let unique_id =
                    ClanUniqueId::new(QuestTriggerHash::from(name.as_str()).hash).unwrap();
                let members = vec![ClanMember::Online {
                    entity: *creator_entity,
                    position: ClanMemberPosition::Master,
                    contribution: ClanPoints(0),
                }];
                let clan_entity = commands
                    .spawn(Clan {
                        unique_id,
                        name: clan_storage.name.clone(),
                        description: clan_storage.description,
                        mark: clan_storage.mark,
                        money: clan_storage.money,
                        points: clan_storage.points,
                        level: clan_storage.level,
                        skills: clan_storage.skills,
                        members,
                    })
                    .id();

                // Add clan membership to creator
                commands
                    .entity(*creator_entity)
                    .insert(ClanMembership::new(clan_entity));

                if let Some(game_client) = creator.game_client {
                    game_client
                        .server_message_tx
                        .send(ServerMessage::UpdateMoney {
                            money: creator.inventory.money,
                        })
                        .ok();
                }

                // Update clan to nearby entities
                server_messages.send_entity_message(
                    creator.client_entity,
                    ServerMessage::CharacterUpdateClan {
                        client_entity_id: creator.client_entity.id,
                        id: unique_id,
                        mark: clan_storage.mark,
                        level: clan_storage.level,
                        name: clan_storage.name,
                        position: ClanMemberPosition::Master,
                    },
                );
            }
            ClanEvent::Invite {
                inviter_entity,
                name: target_name,
            } => {
                // Find inviter's clan
                let Ok(inviter) = query_member.get(*inviter_entity) else {
                    continue;
                };

                let Some(clan_entity) = inviter.clan_membership.clan() else {
                    continue;
                };

                let Ok((_, clan)) = query_clans.get(clan_entity) else {
                    continue;
                };

                // Check inviter permissions (must be >= DeputyMaster)
                let inviter_member = clan.find_online_member(*inviter_entity);
                let has_permission = inviter_member.map_or(false, |m| {
                    matches!(
                        m.position(),
                        ClanMemberPosition::Master | ClanMemberPosition::DeputyMaster
                    )
                });

                if !has_permission {
                    if let Some(game_client) = inviter.game_client {
                        game_client
                            .server_message_tx
                            .send(ServerMessage::ClanInviteResult {
                                response: ClanInviteResponse::NoPermission,
                            })
                            .ok();
                    }
                    continue;
                }

                // Check if clan is full
                if clan.members.len() >= clan_max_members(clan.level) {
                    if let Some(game_client) = inviter.game_client {
                        game_client
                            .server_message_tx
                            .send(ServerMessage::ClanInviteResult {
                                response: ClanInviteResponse::Full,
                            })
                            .ok();
                    }
                    continue;
                }

                // Find target player by name
                let mut target_found = false;
                for target in query_member.iter() {
                    if target.character_info.name == *target_name {
                        target_found = true;

                        // Check if target already has a clan
                        if target.clan_membership.is_some() {
                            if let Some(game_client) = inviter.game_client {
                                game_client
                                    .server_message_tx
                                    .send(ServerMessage::ClanInviteResult {
                                        response: ClanInviteResponse::TargetHasClan,
                                    })
                                    .ok();
                            }
                            break;
                        }

                        // Send invite notification to target
                        if let Some(target_game_client) = target.game_client {
                            if let Some(inviter_client_entity) = inviter.client_entity {
                                target_game_client
                                    .server_message_tx
                                    .send(ServerMessage::ClanInvited {
                                        name: inviter.character_info.name.clone(),
                                        clan_unique_id: clan.unique_id,
                                        clan_mark: clan.mark,
                                        clan_level: clan.level,
                                        clan_name: clan.name.clone(),
                                        inviter_entity_id: inviter_client_entity.id,
                                    })
                                    .ok();
                            }
                        }
                        break;
                    }
                }

                if !target_found {
                    if let Some(game_client) = inviter.game_client {
                        game_client
                            .server_message_tx
                            .send(ServerMessage::ClanInviteResult {
                                response: ClanInviteResponse::TargetNotFound,
                            })
                            .ok();
                    }
                }
            }
            ClanEvent::AcceptInvite {
                invited_entity,
                inviter_name,
            } => {
                // Find the inviter by name
                let mut inviter_entity = None;
                for member in query_member.iter() {
                    if member.character_info.name == *inviter_name {
                        inviter_entity = Some(member.entity);
                        break;
                    }
                }

                let Some(inviter_entity) = inviter_entity else {
                    log::warn!(
                        "[ClanAcceptInvite] Could not find inviter '{}'",
                        inviter_name
                    );
                    continue;
                };

                let Ok(inviter) = query_member.get(inviter_entity) else {
                    continue;
                };

                let Some(clan_entity_id) = inviter.clan_membership.clan() else {
                    continue;
                };

                let Ok(invited) = query_member.get(*invited_entity) else {
                    continue;
                };

                // Check if invited already has a clan
                if invited.clan_membership.is_some() {
                    if let Some(game_client) = inviter.game_client {
                        game_client
                            .server_message_tx
                            .send(ServerMessage::ClanInviteResult {
                                response: ClanInviteResponse::TargetHasClan,
                            })
                            .ok();
                    }
                    continue;
                }

                let Ok((_, mut clan)) = query_clans.get_mut(clan_entity_id) else {
                    continue;
                };

                // Check if clan is full
                if clan.members.len() >= clan_max_members(clan.level) {
                    if let Some(game_client) = inviter.game_client {
                        game_client
                            .server_message_tx
                            .send(ServerMessage::ClanInviteResult {
                                response: ClanInviteResponse::Full,
                            })
                            .ok();
                    }
                    continue;
                }

                // Add member to clan
                clan.members.push(ClanMember::Online {
                    entity: *invited_entity,
                    position: ClanMemberPosition::Junior,
                    contribution: ClanPoints(0),
                });

                let invited_name = invited.character_info.name.clone();
                log::info!(
                    "[ClanAcceptInvite] Successfully added '{}' to clan '{}' (now {} members)",
                    &invited_name,
                    &clan.name,
                    clan.members.len()
                );

                // Send joined notification to all existing online members
                for member in clan.members.iter() {
                    let &ClanMember::Online {
                        entity: member_entity,
                        ..
                    } = member
                    else {
                        continue;
                    };

                    if member_entity == *invited_entity {
                        continue;
                    }

                    if let Ok(online_member) = query_member.get(member_entity) {
                        if let Some(game_client) = online_member.game_client {
                            game_client
                                .server_message_tx
                                .send(ServerMessage::ClanMemberJoined {
                                    name: invited_name.clone(),
                                })
                                .ok();
                        }
                    }
                }

                // Save clan
                save_clan(&clan, &query_member);

                // Set clan membership on the invited entity
                commands
                    .entity(*invited_entity)
                    .insert(ClanMembership::new(clan_entity_id));

                // Notify nearby entities about the new member's clan
                if let Some(client_entity) = invited.client_entity {
                    server_messages.send_entity_message(
                        client_entity,
                        ServerMessage::CharacterUpdateClan {
                            client_entity_id: client_entity.id,
                            id: clan.unique_id,
                            mark: clan.mark,
                            level: clan.level,
                            name: clan.name.clone(),
                            position: ClanMemberPosition::Junior,
                        },
                    );
                }
            }
            ClanEvent::RejectInvite {
                invited_entity: _,
                inviter_name,
            } => {
                // Find inviter by name and notify them
                for member in query_member.iter() {
                    if member.character_info.name == *inviter_name {
                        if let Some(game_client) = member.game_client {
                            game_client
                                .server_message_tx
                                .send(ServerMessage::ClanInviteResult {
                                    response: ClanInviteResponse::Rejected,
                                })
                                .ok();
                        }
                        break;
                    }
                }
            }
            ClanEvent::Kick {
                kicker_entity,
                name: kick_name,
            } => {
                let Ok(kicker) = query_member.get(*kicker_entity) else {
                    continue;
                };

                let Some(clan_entity_id) = kicker.clan_membership.clan() else {
                    continue;
                };

                let Ok((_, mut clan)) = query_clans.get_mut(clan_entity_id) else {
                    continue;
                };

                // Check kicker permissions (must be >= DeputyMaster)
                let kicker_member = clan.find_online_member(*kicker_entity);
                let has_permission = kicker_member.map_or(false, |m| {
                    matches!(
                        m.position(),
                        ClanMemberPosition::Master | ClanMemberPosition::DeputyMaster
                    )
                });

                if !has_permission {
                    continue;
                }

                // Cannot kick the master
                let kick_target_position = clan
                    .members
                    .iter()
                    .find(|m| match m {
                        ClanMember::Online { entity, .. } => query_member
                            .get(*entity)
                            .map_or(false, |q| q.character_info.name == *kick_name),
                        ClanMember::Offline { name, .. } => name == kick_name,
                    })
                    .map(|m| m.position());

                if kick_target_position == Some(ClanMemberPosition::Master) {
                    continue;
                }

                // Find and remove the member
                let mut kicked_entity: Option<Entity> = None;
                clan.members.retain(|member| match member {
                    ClanMember::Online { entity, .. } => {
                        if query_member
                            .get(*entity)
                            .map_or(false, |q| q.character_info.name == *kick_name)
                        {
                            kicked_entity = Some(*entity);
                            false
                        } else {
                            true
                        }
                    }
                    ClanMember::Offline { name, .. } => name != kick_name,
                });

                // Notify the kicked player (if online)
                if let Some(kicked) = kicked_entity {
                    commands.entity(kicked).insert(ClanMembership(None));

                    if let Ok(kicked_member) = query_member.get(kicked) {
                        if let Some(game_client) = kicked_member.game_client {
                            game_client
                                .server_message_tx
                                .send(ServerMessage::ClanKicked)
                                .ok();
                        }
                    }
                }

                // Notify remaining members
                for member in clan.members.iter() {
                    let &ClanMember::Online {
                        entity: member_entity,
                        ..
                    } = member
                    else {
                        continue;
                    };

                    if let Ok(online_member) = query_member.get(member_entity) {
                        if let Some(game_client) = online_member.game_client {
                            game_client
                                .server_message_tx
                                .send(ServerMessage::ClanMemberKicked {
                                    name: kick_name.clone(),
                                })
                                .ok();
                        }
                    }
                }

                // Save clan
                save_clan(&clan, &query_member);
            }
            ClanEvent::Promote {
                changer_entity,
                name: target_name,
            } => {
                let Ok(changer) = query_member.get(*changer_entity) else {
                    continue;
                };

                let Some(clan_entity_id) = changer.clan_membership.clan() else {
                    continue;
                };

                let Ok((_, mut clan)) = query_clans.get_mut(clan_entity_id) else {
                    continue;
                };

                let Some(changer_position) = clan
                    .find_online_member(*changer_entity)
                    .map(|member| member.position())
                else {
                    continue;
                };

                let mut target_online_entity = None;
                let mut promoted_position = None;
                for member in clan.members.iter_mut() {
                    let is_target_member = match member {
                        ClanMember::Online { entity, .. } => query_member
                            .get(*entity)
                            .map_or(false, |q| q.character_info.name == *target_name),
                        ClanMember::Offline { name, .. } => name == target_name,
                    };

                    if !is_target_member {
                        continue;
                    }

                    let Some(next_position) =
                        get_promoted_position(changer_position, member.position())
                    else {
                        break;
                    };

                    match member {
                        ClanMember::Online {
                            entity, position, ..
                        } => {
                            *position = next_position;
                            target_online_entity = Some(*entity);
                        }
                        ClanMember::Offline { position, .. } => {
                            *position = next_position;
                        }
                    }

                    promoted_position = Some(next_position);
                    break;
                }

                let Some(promoted_position) = promoted_position else {
                    continue;
                };

                save_clan(&clan, &query_member);
                send_clan_member_list_to_online_members(&clan, &query_member);

                if let Some(target_online_entity) = target_online_entity {
                    if let Ok(target_member) = query_member.get(target_online_entity) {
                        if let (Some(target_game_client), Some(target_client_entity)) =
                            (target_member.game_client, target_member.client_entity)
                        {
                            let update_message = ServerMessage::CharacterUpdateClan {
                                client_entity_id: target_client_entity.id,
                                id: clan.unique_id,
                                name: clan.name.clone(),
                                mark: clan.mark,
                                level: clan.level,
                                position: promoted_position,
                            };

                            target_game_client
                                .server_message_tx
                                .send(update_message.clone())
                                .ok();
                            server_messages
                                .send_entity_message(target_client_entity, update_message);
                        }
                    }
                }
            }
            ClanEvent::Demote {
                changer_entity,
                name: target_name,
            } => {
                let Ok(changer) = query_member.get(*changer_entity) else {
                    continue;
                };

                let Some(clan_entity_id) = changer.clan_membership.clan() else {
                    continue;
                };

                let Ok((_, mut clan)) = query_clans.get_mut(clan_entity_id) else {
                    continue;
                };

                let Some(changer_position) = clan
                    .find_online_member(*changer_entity)
                    .map(|member| member.position())
                else {
                    continue;
                };

                let mut target_online_entity = None;
                let mut demoted_position = None;
                for member in clan.members.iter_mut() {
                    let is_target_member = match member {
                        ClanMember::Online { entity, .. } => query_member
                            .get(*entity)
                            .map_or(false, |q| q.character_info.name == *target_name),
                        ClanMember::Offline { name, .. } => name == target_name,
                    };

                    if !is_target_member {
                        continue;
                    }

                    let Some(next_position) =
                        get_demoted_position(changer_position, member.position())
                    else {
                        break;
                    };

                    match member {
                        ClanMember::Online {
                            entity, position, ..
                        } => {
                            *position = next_position;
                            target_online_entity = Some(*entity);
                        }
                        ClanMember::Offline { position, .. } => {
                            *position = next_position;
                        }
                    }

                    demoted_position = Some(next_position);
                    break;
                }

                let Some(demoted_position) = demoted_position else {
                    continue;
                };

                save_clan(&clan, &query_member);
                send_clan_member_list_to_online_members(&clan, &query_member);

                if let Some(target_online_entity) = target_online_entity {
                    if let Ok(target_member) = query_member.get(target_online_entity) {
                        if let (Some(target_game_client), Some(target_client_entity)) =
                            (target_member.game_client, target_member.client_entity)
                        {
                            let update_message = ServerMessage::CharacterUpdateClan {
                                client_entity_id: target_client_entity.id,
                                id: clan.unique_id,
                                name: clan.name.clone(),
                                mark: clan.mark,
                                level: clan.level,
                                position: demoted_position,
                            };

                            target_game_client
                                .server_message_tx
                                .send(update_message.clone())
                                .ok();
                            server_messages
                                .send_entity_message(target_client_entity, update_message);
                        }
                    }
                }
            }
            ClanEvent::Upgrade {
                requester_entity,
                npc_entity_id,
            } => {
                let Ok(requester) = query_member.get(*requester_entity) else {
                    continue;
                };

                let Some(clan_entity_id) = requester.clan_membership.clan() else {
                    send_clan_upgrade_result(&requester, ClanUpgradeResult::NoClan);
                    continue;
                };

                let Some((_, _, npc_position)) = find_npc_entity(&query_npc, *npc_entity_id) else {
                    send_clan_upgrade_result(&requester, ClanUpgradeResult::InvalidNpc);
                    continue;
                };

                if requester.position.zone_id != npc_position.zone_id
                    || requester
                        .position
                        .position
                        .xy()
                        .distance(npc_position.position.xy())
                        > 6000.0
                {
                    send_clan_upgrade_result(&requester, ClanUpgradeResult::NpcTooFar);
                    continue;
                }

                let Ok((_, mut clan)) = query_clans.get_mut(clan_entity_id) else {
                    send_clan_upgrade_result(&requester, ClanUpgradeResult::NoClan);
                    continue;
                };

                let is_master = clan
                    .find_online_member(*requester_entity)
                    .map_or(false, |member| {
                        member.position() == ClanMemberPosition::Master
                    });
                if !is_master {
                    send_clan_upgrade_result(&requester, ClanUpgradeResult::NoPermission);
                    continue;
                }

                if clan.level.get() >= MAX_CLAN_LEVEL {
                    send_clan_upgrade_result(&requester, ClanUpgradeResult::MaxLevel);
                    continue;
                }

                let next_level = clan.level.get() + 1;
                let Some(required_points) = game_config.clan_upgrade_points_required(next_level)
                else {
                    send_clan_upgrade_result(&requester, ClanUpgradeResult::MaxLevel);
                    continue;
                };
                if clan.points.0 < required_points.0 {
                    send_clan_upgrade_result(&requester, ClanUpgradeResult::InsufficientPoints);
                    continue;
                }

                let Some(next_level) = ClanLevel::new(next_level) else {
                    send_clan_upgrade_result(&requester, ClanUpgradeResult::MaxLevel);
                    continue;
                };

                clan.level = next_level;
                send_update_clan_info(&clan, &query_member);
                send_character_update_clan_for_online_members(
                    &clan,
                    &query_member,
                    &mut server_messages,
                );
                save_clan(&clan, &query_member);
                send_clan_upgrade_result(&requester, ClanUpgradeResult::Success);
            }
            ClanEvent::Leave { leaver_entity } => {
                let Ok(leaver) = query_member.get(*leaver_entity) else {
                    continue;
                };

                let Some(clan_entity_id) = leaver.clan_membership.clan() else {
                    continue;
                };

                let Ok((_, mut clan)) = query_clans.get_mut(clan_entity_id) else {
                    continue;
                };

                // Check if leaver is master
                let is_master = clan.find_online_member(*leaver_entity).map_or(false, |m| {
                    matches!(m.position(), ClanMemberPosition::Master)
                });

                // Master cannot leave unless they are the last member
                if is_master && clan.members.len() > 1 {
                    continue;
                }

                let leaver_name = leaver.character_info.name.clone();

                // Remove from clan members
                clan.members
                    .retain(|member| !matches!(member, ClanMember::Online { entity, .. } if *entity == *leaver_entity));

                // Clear clan membership
                commands.entity(*leaver_entity).insert(ClanMembership(None));

                // If no members remain, delete the clan
                if clan.members.is_empty() {
                    let clan_name = clan.name.clone();
                    commands.entity(clan_entity_id).despawn();

                    if let Err(error) = ClanStorage::delete(&clan_name) {
                        log::error!(
                            "Failed to delete clan storage for {}: {:?}",
                            clan_name,
                            error
                        );
                    }

                    // Notify the leaver that the clan is disbanded
                    if let Some(game_client) = leaver.game_client {
                        game_client
                            .server_message_tx
                            .send(ServerMessage::ClanDisbanded)
                            .ok();
                    }
                } else {
                    // Notify the leaver that they left the clan
                    if let Some(game_client) = leaver.game_client {
                        game_client
                            .server_message_tx
                            .send(ServerMessage::ClanDisbanded)
                            .ok();
                    }

                    // Notify remaining members
                    for member in clan.members.iter() {
                        let &ClanMember::Online {
                            entity: member_entity,
                            ..
                        } = member
                        else {
                            continue;
                        };

                        if let Ok(online_member) = query_member.get(member_entity) {
                            if let Some(game_client) = online_member.game_client {
                                game_client
                                    .server_message_tx
                                    .send(ServerMessage::ClanMemberLeft {
                                        name: leaver_name.clone(),
                                    })
                                    .ok();
                            }
                        }
                    }

                    // Save clan
                    save_clan(&clan, &query_member);
                }
            }
            ClanEvent::Disband { entity } => {
                let Ok(disbander) = query_member.get(*entity) else {
                    continue;
                };

                let Some(clan_entity_id) = disbander.clan_membership.clan() else {
                    continue;
                };

                let Ok((_, mut clan)) = query_clans.get_mut(clan_entity_id) else {
                    continue;
                };

                // Only master can disband
                let is_master = clan.find_online_member(*entity).map_or(false, |m| {
                    matches!(m.position(), ClanMemberPosition::Master)
                });

                if !is_master {
                    continue;
                }

                // Notify and clear all members
                for member in clan.members.iter() {
                    let &ClanMember::Online {
                        entity: member_entity,
                        ..
                    } = member
                    else {
                        continue;
                    };

                    commands.entity(member_entity).insert(ClanMembership(None));

                    if let Ok(online_member) = query_member.get(member_entity) {
                        if let Some(game_client) = online_member.game_client {
                            game_client
                                .server_message_tx
                                .send(ServerMessage::ClanDisbanded)
                                .ok();
                        }
                    }
                }

                // Delete clan storage file
                let clan_name = clan.name.clone();
                clan.members.clear();

                // Remove clan entity
                commands.entity(clan_entity_id).despawn();

                // Delete storage file
                if let Err(error) = ClanStorage::delete(&clan_name) {
                    log::error!(
                        "Failed to delete clan storage for {}: {:?}",
                        clan_name,
                        error
                    );
                }
            }
            &ClanEvent::MemberDisconnect {
                clan_entity,
                disconnect_entity,
                ref name,
                level,
                job,
            } => {
                // Find the right clan by matching the entity
                if let Ok((_, mut clan)) = query_clans.get_mut(clan_entity) {
                    if let Some(clan_member) = clan.find_online_member_mut(disconnect_entity) {
                        let &mut ClanMember::Online {
                            position,
                            contribution,
                            ..
                        } = clan_member
                        else {
                            unreachable!()
                        };
                        *clan_member = ClanMember::Offline {
                            name: name.clone(),
                            position,
                            contribution,
                            level,
                            job,
                        };

                        // Send message to other clan members that we have disconnected
                        for clan_member in clan.members.iter() {
                            let &ClanMember::Online {
                                entity: clan_member_entity,
                                ..
                            } = clan_member
                            else {
                                continue;
                            };

                            if let Ok(online_member) = query_member.get(clan_member_entity) {
                                if let Some(online_member_game_client) = online_member.game_client {
                                    online_member_game_client
                                        .server_message_tx
                                        .send(ServerMessage::ClanMemberDisconnected {
                                            name: name.clone(),
                                        })
                                        .ok();
                                }
                            }
                        }

                        // Save clan on member disconnect
                        save_clan(&clan, &query_member);
                    }
                }
            }
            &ClanEvent::GetMemberList { entity } => {
                if let Ok(requestor) = query_member.get(entity) {
                    if let Some(clan_entity_id) = requestor.clan_membership.clan() {
                        if let Ok((_, clan)) = query_clans.get(clan_entity_id) {
                            if let Some(game_client) = requestor.game_client {
                                game_client
                                    .server_message_tx
                                    .send(ServerMessage::ClanMemberList {
                                        members: build_clan_member_list(&clan, &query_member),
                                    })
                                    .ok();
                            }
                        }
                    }
                }
            }
            ClanEvent::SetDescription {
                updater_entity,
                description,
            } => {
                let Ok(updater) = query_member.get(*updater_entity) else {
                    continue;
                };

                let Some(clan_entity_id) = updater.clan_membership.clan() else {
                    continue;
                };

                let Ok((_, mut clan)) = query_clans.get_mut(clan_entity_id) else {
                    continue;
                };

                let is_master = clan
                    .find_online_member(*updater_entity)
                    .map_or(false, |member| {
                        matches!(member.position(), ClanMemberPosition::Master)
                    });

                if !is_master {
                    continue;
                }

                clan.description = description.clone();
                save_clan(&clan, &query_member);
                send_update_clan_info(&clan, &query_member);
            }
            &ClanEvent::AddLevel { clan_entity, level } => {
                if let Ok((_, mut clan)) = query_clans.get_mut(clan_entity) {
                    if let Some(level) = clan
                        .level
                        .0
                        .get()
                        .checked_add_signed(level)
                        .and_then(NonZeroU32::new)
                    {
                        clan.level = ClanLevel(level);
                        send_update_clan_info(&clan, &query_member);
                        send_character_update_clan_for_online_members(
                            &clan,
                            &query_member,
                            &mut server_messages,
                        );
                        save_clan(&clan, &query_member);
                    }
                }
            }
            &ClanEvent::SetLevel { clan_entity, level } => {
                if let Ok((_, mut clan)) = query_clans.get_mut(clan_entity) {
                    clan.level = level;
                    send_update_clan_info(&clan, &query_member);
                    send_character_update_clan_for_online_members(
                        &clan,
                        &query_member,
                        &mut server_messages,
                    );
                    save_clan(&clan, &query_member);
                }
            }
            &ClanEvent::AddMoney { clan_entity, money } => {
                if let Ok((_, mut clan)) = query_clans.get_mut(clan_entity) {
                    if let Some(money) = clan.money.0.checked_add(money) {
                        clan.money = Money(money);
                        send_update_clan_info(&clan, &query_member);
                        save_clan(&clan, &query_member);
                    }
                }
            }
            &ClanEvent::SetMoney { clan_entity, money } => {
                if let Ok((_, mut clan)) = query_clans.get_mut(clan_entity) {
                    clan.money = money;
                    send_update_clan_info(&clan, &query_member);
                    save_clan(&clan, &query_member);
                }
            }
            &ClanEvent::AddPoints {
                clan_entity,
                points,
            } => {
                if let Ok((_, mut clan)) = query_clans.get_mut(clan_entity) {
                    if let Some(points) = clan.points.0.checked_add_signed(points) {
                        clan.points = ClanPoints(points);
                        send_update_clan_info(&clan, &query_member);
                        save_clan(&clan, &query_member);
                    }
                }
            }
            &ClanEvent::SetPoints {
                clan_entity,
                points,
            } => {
                if let Ok((_, mut clan)) = query_clans.get_mut(clan_entity) {
                    clan.points = points;
                    send_update_clan_info(&clan, &query_member);
                    save_clan(&clan, &query_member);
                }
            }
            &ClanEvent::AddSkill {
                clan_entity,
                skill_id,
            } => {
                if let Ok((_, mut clan)) = query_clans.get_mut(clan_entity) {
                    if !clan.skills.iter().any(|id| *id == skill_id) {
                        clan.skills.push(skill_id);
                        send_update_clan_info(&clan, &query_member);
                        save_clan(&clan, &query_member);
                    }
                }
            }
            &ClanEvent::RemoveSkill {
                clan_entity,
                skill_id,
            } => {
                if let Ok((_, mut clan)) = query_clans.get_mut(clan_entity) {
                    if clan.skills.iter().any(|id| *id == skill_id) {
                        clan.skills.retain(|id| *id != skill_id);
                        send_update_clan_info(&clan, &query_member);
                        save_clan(&clan, &query_member);
                    }
                }
            }
        }
    }

    for connected_member in query_member_connected.iter() {
        let Some(clan_entity_id) = connected_member.clan_membership.clan() else {
            continue;
        };

        let Ok((_, clan)) = query_clans.get(clan_entity_id) else {
            continue;
        };

        let Some(&ClanMember::Online {
            position: connected_member_position,
            contribution: connected_member_contribution,
            ..
        }) = clan.find_online_member(connected_member.entity)
        else {
            continue;
        };

        if let Some(game_client) = connected_member.game_client.as_ref() {
            game_client
                .server_message_tx
                .send(ServerMessage::ClanInfo {
                    id: clan.unique_id,
                    name: clan.name.clone(),
                    description: clan.description.clone(),
                    mark: clan.mark,
                    level: clan.level,
                    points: clan.points,
                    money: clan.money,
                    skills: clan.skills.clone(),
                    position: connected_member_position,
                    contribution: connected_member_contribution,
                })
                .ok();
        }

        // Send message to other clan members that we have connected
        for clan_member in clan.members.iter() {
            let &ClanMember::Online {
                entity: clan_member_entity,
                ..
            } = clan_member
            else {
                continue;
            };

            if clan_member_entity == connected_member.entity {
                continue;
            }

            if let Ok(online_member) = query_member.get(clan_member_entity) {
                if let Some(online_member_game_client) = online_member.game_client {
                    online_member_game_client
                        .server_message_tx
                        .send(ServerMessage::ClanMemberConnected {
                            name: connected_member.character_info.name.clone(),
                            channel_id: NonZeroUsize::new(1).unwrap(),
                        })
                        .ok();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::Once,
        time::{SystemTime, UNIX_EPOCH},
    };

    use bevy::{
        math::Vec3,
        prelude::{App, Update},
    };
    use crossbeam_channel::unbounded as crossbeam_unbounded;
    use rose_game_common::{
        components::{get_default_clan_upgrade_points, CharacterGender, Money},
        messages::{
            server::{ClanUpgradeResult, ServerMessage},
            ClientEntityId,
        },
    };
    use tokio::sync::mpsc::{error::TryRecvError, unbounded_channel, UnboundedReceiver};

    use super::clan_system;
    use crate::game::{
        components::{
            CharacterInfo, Clan, ClanMembership, ClientEntity, ClientEntityType, GameClient,
            Inventory, Level, Npc, Position,
        },
        events::ClanEvent,
        resources::{GameConfig, ServerMessages},
        storage::clan::ClanStorage,
    };

    static TEST_STORAGE_INIT: Once = Once::new();

    fn create_test_app() -> App {
        let mut app = App::new();
        app.add_event::<ClanEvent>();
        app.insert_resource(GameConfig::default());
        app.insert_resource(ServerMessages::default());
        app.add_systems(Update, clan_system);
        app
    }

    fn setup_test_storage_dir() {
        TEST_STORAGE_INIT.call_once(|| {
            let storage_dir = std::env::current_dir()
                .unwrap()
                .join("target")
                .join("test-clan-system-data");
            std::fs::create_dir_all(&storage_dir).unwrap();
            std::env::set_var("ROSE_OFFLINE_TEST_DATA_DIR", storage_dir);
        });
    }

    fn unique_clan_name(prefix: &str) -> String {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{}_{}", prefix, suffix)
    }

    fn test_character_info(name: &str) -> CharacterInfo {
        CharacterInfo {
            name: name.to_string(),
            gender: CharacterGender::Male,
            race: 0,
            birth_stone: 0,
            job: 111,
            face: 0,
            hair: 0,
            rank: 0,
            fame: 0,
            fame_b: 0,
            fame_g: 0,
            revive_zone_id: rose_data::ZoneId::new(1).unwrap(),
            revive_position: Vec3::ZERO,
            unique_id: 1,
        }
    }

    fn spawn_creator(
        app: &mut App,
        name: &str,
        money: Money,
    ) -> (bevy::prelude::Entity, UnboundedReceiver<ServerMessage>) {
        let (client_message_tx, client_message_rx) = crossbeam_unbounded();
        drop(client_message_tx);
        let (server_message_tx, server_message_rx) = unbounded_channel();

        let mut inventory = Inventory::default();
        inventory.money = money;

        let creator = app
            .world
            .spawn((
                ClientEntity::new(
                    ClientEntityType::Character,
                    ClientEntityId(1),
                    rose_data::ZoneId::new(1).unwrap(),
                ),
                test_character_info(name),
                Position::new(Vec3::ZERO, rose_data::ZoneId::new(1).unwrap()),
                Level::new(30),
                inventory,
                GameClient::new(client_message_rx, server_message_tx),
                ClanMembership::default(),
            ))
            .id();

        (creator, server_message_rx)
    }

    #[test]
    fn clan_create_updates_creator_money_after_success() {
        setup_test_storage_dir();
        let clan_name = unique_clan_name("test_clan_create_success");
        ClanStorage::delete(&clan_name).ok();

        let mut app = create_test_app();
        let (creator, mut server_message_rx) =
            spawn_creator(&mut app, "CreatorSuccess", Money(1_500_000));

        app.world.send_event(ClanEvent::Create {
            creator,
            name: clan_name.clone(),
            description: "Integration test clan".to_string(),
            mark: rose_game_common::components::ClanMark::Premade {
                background: std::num::NonZeroU16::new(1).unwrap(),
                foreground: std::num::NonZeroU16::new(1).unwrap(),
            },
            skip_requirements: false,
        });
        app.update();

        assert_eq!(
            app.world.get::<Inventory>(creator).unwrap().money,
            Money(500_000)
        );

        let clan_membership = app.world.get::<ClanMembership>(creator).unwrap();
        let clan_entity = clan_membership.clan().unwrap();
        let clan = app.world.get::<Clan>(clan_entity).unwrap();
        assert_eq!(clan.name, clan_name);

        match server_message_rx.try_recv() {
            Ok(ServerMessage::UpdateMoney { money }) => {
                assert_eq!(money, Money(500_000));
            }
            other => panic!(
                "expected UpdateMoney after clan create success, got {:?}",
                other
            ),
        }
        assert!(matches!(
            server_message_rx.try_recv(),
            Err(TryRecvError::Empty)
        ));

        ClanStorage::delete(&clan_name).ok();
    }

    #[test]
    fn clan_create_with_insufficient_money_keeps_balance_and_sends_error() {
        setup_test_storage_dir();
        let clan_name = unique_clan_name("test_clan_create_failure");
        ClanStorage::delete(&clan_name).ok();

        let mut app = create_test_app();
        let (creator, mut server_message_rx) =
            spawn_creator(&mut app, "CreatorFailure", Money(999_999));

        app.world.send_event(ClanEvent::Create {
            creator,
            name: clan_name.clone(),
            description: "Should not be created".to_string(),
            mark: rose_game_common::components::ClanMark::Premade {
                background: std::num::NonZeroU16::new(1).unwrap(),
                foreground: std::num::NonZeroU16::new(1).unwrap(),
            },
            skip_requirements: false,
        });
        app.update();

        assert_eq!(
            app.world.get::<Inventory>(creator).unwrap().money,
            Money(999_999)
        );
        assert!(app
            .world
            .get::<ClanMembership>(creator)
            .unwrap()
            .clan()
            .is_none());
        assert!(!ClanStorage::exists(&clan_name));

        match server_message_rx.try_recv() {
            Ok(ServerMessage::ClanCreateError { error }) => {
                assert!(matches!(
                    error,
                    rose_game_common::messages::server::ClanCreateError::UnmetCondition
                ));
            }
            other => panic!(
                "expected ClanCreateError after insufficient money, got {:?}",
                other
            ),
        }
        assert!(matches!(
            server_message_rx.try_recv(),
            Err(TryRecvError::Empty)
        ));
    }

    fn spawn_npc(
        app: &mut App,
        entity_id: ClientEntityId,
        position: Vec3,
    ) -> bevy::prelude::Entity {
        app.world
            .spawn((
                ClientEntity::new(
                    ClientEntityType::Npc,
                    entity_id,
                    rose_data::ZoneId::new(1).unwrap(),
                ),
                Position::new(position, rose_data::ZoneId::new(1).unwrap()),
                Npc::new(rose_data::NpcId::new(1).unwrap(), 0),
            ))
            .id()
    }

    fn drain_server_messages(
        server_message_rx: &mut UnboundedReceiver<ServerMessage>,
    ) -> Vec<ServerMessage> {
        let mut messages = Vec::new();
        loop {
            match server_message_rx.try_recv() {
                Ok(message) => messages.push(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        messages
    }

    #[test]
    fn clan_upgrade_succeeds_at_required_points_threshold() {
        setup_test_storage_dir();
        let clan_name = unique_clan_name("test_clan_upgrade_success");
        ClanStorage::delete(&clan_name).ok();

        let mut app = create_test_app();
        let (creator, mut server_message_rx) =
            spawn_creator(&mut app, "UpgradeSuccess", Money(1_500_000));

        app.world.send_event(ClanEvent::Create {
            creator,
            name: clan_name.clone(),
            description: "Upgrade success test".to_string(),
            mark: rose_game_common::components::ClanMark::Premade {
                background: std::num::NonZeroU16::new(1).unwrap(),
                foreground: std::num::NonZeroU16::new(1).unwrap(),
            },
            skip_requirements: true,
        });
        app.update();

        let clan_entity = app
            .world
            .get::<ClanMembership>(creator)
            .unwrap()
            .clan()
            .unwrap();
        let required_points = get_default_clan_upgrade_points(2).unwrap();
        app.world.get_mut::<Clan>(clan_entity).unwrap().points = required_points;

        let npc_entity_id = ClientEntityId(700);
        spawn_npc(&mut app, npc_entity_id, Vec3::new(5.0, 0.0, 0.0));
        drain_server_messages(&mut server_message_rx);

        app.world.send_event(ClanEvent::Upgrade {
            requester_entity: creator,
            npc_entity_id,
        });
        app.update();

        let clan = app.world.get::<Clan>(clan_entity).unwrap();
        assert_eq!(clan.level.get(), 2);
        assert_eq!(clan.points.0, required_points.0);

        let messages = drain_server_messages(&mut server_message_rx);
        assert!(messages.iter().any(|message| matches!(
            message,
            ServerMessage::ClanUpgradeResult {
                result: ClanUpgradeResult::Success
            }
        )));
        assert!(messages.iter().any(|message| matches!(
            message,
            ServerMessage::ClanUpdateInfo { level, .. } if level.get() == 2
        )));
        assert!(messages.iter().any(|message| matches!(
            message,
            ServerMessage::CharacterUpdateClan { level, .. } if level.get() == 2
        )));

        ClanStorage::delete(&clan_name).ok();
    }

    #[test]
    fn clan_upgrade_rejects_when_points_are_insufficient() {
        setup_test_storage_dir();
        let clan_name = unique_clan_name("test_clan_upgrade_insufficient");
        ClanStorage::delete(&clan_name).ok();

        let mut app = create_test_app();
        let (creator, mut server_message_rx) =
            spawn_creator(&mut app, "UpgradeFailure", Money(1_500_000));

        app.world.send_event(ClanEvent::Create {
            creator,
            name: clan_name.clone(),
            description: "Upgrade failure test".to_string(),
            mark: rose_game_common::components::ClanMark::Premade {
                background: std::num::NonZeroU16::new(1).unwrap(),
                foreground: std::num::NonZeroU16::new(1).unwrap(),
            },
            skip_requirements: true,
        });
        app.update();

        let clan_entity = app
            .world
            .get::<ClanMembership>(creator)
            .unwrap()
            .clan()
            .unwrap();
        let required_points = get_default_clan_upgrade_points(2).unwrap();
        app.world.get_mut::<Clan>(clan_entity).unwrap().points =
            rose_game_common::components::ClanPoints(required_points.0 - 1);

        let npc_entity_id = ClientEntityId(701);
        spawn_npc(&mut app, npc_entity_id, Vec3::new(5.0, 0.0, 0.0));
        drain_server_messages(&mut server_message_rx);

        app.world.send_event(ClanEvent::Upgrade {
            requester_entity: creator,
            npc_entity_id,
        });
        app.update();

        let clan = app.world.get::<Clan>(clan_entity).unwrap();
        assert_eq!(clan.level.get(), 1);

        let messages = drain_server_messages(&mut server_message_rx);
        assert!(messages.iter().any(|message| matches!(
            message,
            ServerMessage::ClanUpgradeResult {
                result: ClanUpgradeResult::InsufficientPoints
            }
        )));
        assert!(!messages.iter().any(|message| matches!(
            message,
            ServerMessage::ClanUpdateInfo { level, .. } if level.get() > 1
        )));

        ClanStorage::delete(&clan_name).ok();
    }
}
