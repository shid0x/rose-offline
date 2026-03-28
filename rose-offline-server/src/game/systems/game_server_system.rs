use bevy::{
    ecs::{
        prelude::{Commands, Entity, EventWriter, Query, Res, ResMut, Without, World},
        query::WorldQuery,
        system::SystemParam,
    },
    math::{Vec3, Vec3Swizzles},
    time::Time,
};
use log::warn;
use std::collections::HashSet;

use rose_data::{
    AbilityType, EquipmentIndex, EquipmentItem, Item, ItemClass, ItemReference, ItemSlotBehaviour,
    ItemType, SkillType, StackableItem, VehiclePartIndex,
};
use rose_game_common::{
    components::CharacterUniqueId,
    data::Password,
    data::{
        disassemble_from_npc_price, manufacture_required_mp, manufacture_success_chance,
        upgrade_from_npc_price,
    },
    messages::{
        server::{CharacterData, CharacterDataItems, CraftCreateItemError, CraftInsertGemError},
        Friend, FriendInfo, FriendStatus,
    },
};

use crate::game::{
    bundles::{
        client_entity_join_zone, client_entity_leave_zone, client_entity_teleport_zone,
        skill_list_try_level_up_skill, CharacterBundle, ItemDropBundle, SkillListBundle,
    },
    components::{
        AbilityValues, Account, Bank, BasicStatType, BasicStats, CharacterInfo, Clan, ClanMember,
        ClanMembership, ClientEntity, ClientEntitySector, ClientEntityType, ClientEntityVisibility,
        Command, CommandData, Cooldowns, DamageSources, Dead, DrivingTime, DroppedItem, Equipment,
        EquipmentItemDatabase, ExperiencePoints, FriendList, GameClient, HealthPoints, Hotbar,
        Inventory, ItemSlot, Level, ManaPoints, Money, MotionData, MoveMode, MoveSpeed,
        NextCommand, Party, PartyMember, PartyMembership, PassiveRecoveryTime, PersonalStore,
        Position, QuestState, RecoveryRateBonus, SkillList, SkillPoints, StatPoints, StatusEffects,
        StatusEffectsRegen, Team, WorldClient,
    },
    events::{
        BankEvent, ChatCommandEvent, ClanEvent, EquipmentEvent, ItemLifeEvent, NpcStoreEvent,
        PartyEvent, PartyMemberEvent, PersonalStoreEvent, QuestTriggerEvent, ReviveEvent,
        RevivePosition, UseItemEvent,
    },
    messages::{
        client::ClientMessage,
        server::{ConnectionRequestError, ServerMessage},
    },
    pvp::join_zone_global_flags,
    resources::{
        ClientEntityList, GameData, LoginTokens, OnlineFriends, ServerMessages, WorldRates,
        WorldTime,
    },
    storage::{account::AccountStorage, bank::BankStorage, character::CharacterStorage},
};

#[derive(Copy, Clone)]
enum ResolvedCraftMaterialRequirement {
    Item(ItemReference),
    ItemClass(ItemClass),
    Unknown,
}

#[derive(Copy, Clone)]
struct UpgradeMaterialRequirement {
    quantity: u32,
    requirement: ResolvedCraftMaterialRequirement,
}

fn resolve_craft_material_requirement(
    game_data: &GameData,
    required_item: ItemReference,
) -> ResolvedCraftMaterialRequirement {
    if game_data.items.get_base_item(required_item).is_some() {
        return ResolvedCraftMaterialRequirement::Item(required_item);
    }

    if let Some(item_class) = game_data
        .data_decoder
        .decode_item_class(required_item.item_number)
    {
        return ResolvedCraftMaterialRequirement::ItemClass(item_class);
    }

    ResolvedCraftMaterialRequirement::Unknown
}

fn resolve_upgrade_product_row_id(target_item: ItemReference, grade: u8) -> Option<u32> {
    if !target_item.item_type.is_equipment_item() {
        return None;
    }

    let base_row = match target_item.item_type {
        ItemType::Weapon => 1u32,
        _ => 11u32,
    };
    Some(base_row + grade as u32)
}

fn build_upgrade_material_requirements(
    game_data: &GameData,
    target_item: ItemReference,
    grade: u8,
) -> Option<[Option<UpgradeMaterialRequirement>; 3]> {
    let mut requirements = [None, None, None];
    let product_row_id = resolve_upgrade_product_row_id(target_item, grade)?;
    let product = game_data.products.get_product(product_row_id)?;

    if let Some(material) = product.materials.get(0) {
        let requirement = if product.raw_material_type > 0 {
            game_data
                .data_decoder
                .decode_item_class(product.raw_material_type as usize)
                .map(ResolvedCraftMaterialRequirement::ItemClass)
                .unwrap_or_else(|| resolve_craft_material_requirement(game_data, material.item))
        } else {
            resolve_craft_material_requirement(game_data, material.item)
        };
        requirements[0] = Some(UpgradeMaterialRequirement {
            quantity: material.quantity,
            requirement,
        });
    }

    if let Some(material) = product.materials.get(1) {
        requirements[1] = Some(UpgradeMaterialRequirement {
            quantity: material.quantity,
            requirement: resolve_craft_material_requirement(game_data, material.item),
        });
    }

    if let Some(material) = product.materials.get(2) {
        requirements[2] = Some(UpgradeMaterialRequirement {
            quantity: material.quantity,
            requirement: resolve_craft_material_requirement(game_data, material.item),
        });
    }

    Some(requirements)
}

fn validate_upgrade_materials(
    game_data: &GameData,
    inventory: &Inventory,
    target_item: ItemReference,
    grade: u8,
    ingredients: &[ItemSlot; 3],
) -> Result<[u32; 3], &'static str> {
    let requirements = build_upgrade_material_requirements(game_data, target_item, grade)
        .ok_or("missing upgrade requirement row")?;
    let mut required_quantities = [0u32; 3];

    for (row_index, requirement) in requirements.iter().enumerate() {
        let Some(requirement) = requirement else {
            continue;
        };
        required_quantities[row_index] = requirement.quantity;

        let Some(inv_item) = inventory.get_item(ingredients[row_index]) else {
            return Err("missing ingredient item");
        };

        let has_enough = match (inv_item, requirement.requirement) {
            (Item::Stackable(s), ResolvedCraftMaterialRequirement::Item(item)) => {
                s.item == item && s.quantity >= requirement.quantity
            }
            (Item::Stackable(s), ResolvedCraftMaterialRequirement::ItemClass(required_class)) => {
                s.quantity >= requirement.quantity
                    && game_data
                        .items
                        .get_base_item(s.item)
                        .map_or(false, |item_data| item_data.class == required_class)
            }
            (_, ResolvedCraftMaterialRequirement::Unknown) => false,
            _ => false,
        };

        if !has_enough {
            return Err("ingredient mismatch");
        }
    }

    // Prevent duplicate-slot bypass by applying required consumption on a cloned inventory.
    let mut simulated_inventory = inventory.clone();
    for (row_index, required_quantity) in required_quantities.iter().enumerate() {
        if *required_quantity == 0 {
            continue;
        }
        if simulated_inventory
            .try_take_quantity(ingredients[row_index], *required_quantity)
            .is_none()
        {
            return Err("insufficient aggregate quantity");
        }
    }

    Ok(required_quantities)
}

fn resolve_upgrade_required_mp(game_data: &GameData, skill_list: &SkillList) -> Option<i32> {
    for page in &skill_list.pages {
        for skill_id in page.skills.iter().flatten() {
            let Some(skill_data) = game_data.skills.get_skill(*skill_id) else {
                continue;
            };

            if skill_data.skill_type == SkillType::CreateWindow && skill_data.item_make_number == 42
            {
                return Some(manufacture_required_mp(skill_data));
            }
        }
    }

    None
}

fn collect_craft_material_inventory_updates(
    inventory: &Inventory,
    material_inventory_slots: &[ItemSlot],
) -> Vec<(ItemSlot, Option<Item>)> {
    let mut seen_slots = HashSet::new();
    let mut updates = Vec::new();

    for &slot in material_inventory_slots {
        if seen_slots.insert(slot) {
            updates.push((slot, inventory.get_item(slot).cloned()));
        }
    }

    updates
}

fn contains_friend(friends: &[Friend], character_id: CharacterUniqueId) -> bool {
    friends
        .iter()
        .any(|friend| friend.character_id == character_id)
}

fn friend_status_from_online(
    online_friends: &OnlineFriends,
    character_id: CharacterUniqueId,
) -> FriendStatus {
    if online_friends.get_by_id(character_id).is_some() {
        FriendStatus::Online
    } else {
        FriendStatus::Offline
    }
}

fn build_friend_infos(online_friends: &OnlineFriends, friends: &[Friend]) -> Vec<FriendInfo> {
    friends
        .iter()
        .map(|friend| FriendInfo {
            character_id: friend.character_id,
            name: friend.name.clone(),
            status: friend_status_from_online(online_friends, friend.character_id),
        })
        .collect()
}

fn should_fire_zone_join_trigger(zone_id: rose_data::ZoneId) -> bool {
    // Lion's Plain is an event-only PvP map. Outside its original event flow, the join trigger
    // immediately redirects the player back to Junon, which makes GM/debug teleports unusable.
    zone_id.get() != 8
}

fn send_friend_status_to_online_friends(
    world: &mut World,
    current_id: CharacterUniqueId,
    current_friends: &[Friend],
    status: FriendStatus,
) {
    let recipient_entities = {
        let online_friends = world.resource::<OnlineFriends>();
        current_friends
            .iter()
            .filter_map(|friend| online_friends.get_by_id(friend.character_id))
            .map(|online_friend| online_friend.entity)
            .collect::<Vec<_>>()
    };

    let mut query = world.query::<(&FriendList, &GameClient)>();
    for recipient_entity in recipient_entities {
        if let Ok((recipient_friend_list, recipient_game_client)) =
            query.get(world, recipient_entity)
        {
            if contains_friend(&recipient_friend_list.friends, current_id) {
                recipient_game_client
                    .server_message_tx
                    .send(ServerMessage::FriendStatusChanged {
                        friend_id: current_id,
                        status,
                    })
                    .ok();
            }
        }
    }
}

fn handle_game_connection_request(
    commands: &mut Commands,
    game_data: &GameData,
    login_tokens: &mut LoginTokens,
    online_friends: &mut OnlineFriends,
    entity: Entity,
    game_client: &mut GameClient,
    token_id: u32,
    password: &Password,
    query_world_client: &mut Query<&mut WorldClient>,
    query_clans: &mut Query<(Entity, &mut Clan)>,
) -> Result<
    (
        u32,
        Box<CharacterData>,
        Box<CharacterDataItems>,
        Box<QuestState>,
    ),
    ConnectionRequestError,
> {
    // Verify token
    let login_token = login_tokens
        .get_token_mut(token_id)
        .ok_or(ConnectionRequestError::InvalidToken)?;
    if login_token.world_client.is_none() || login_token.game_client.is_some() {
        return Err(ConnectionRequestError::InvalidToken);
    }

    let mut world_client =
        if let Ok(world_client) = query_world_client.get_mut(login_token.world_client.unwrap()) {
            world_client
        } else {
            return Err(ConnectionRequestError::InvalidToken);
        };

    // Verify account password
    let account: Account = AccountStorage::try_load(&login_token.username, password)
        .map_err(|error| {
            log::error!(
                "Failed to load account {} with error {:?}",
                &login_token.username,
                error
            );
            ConnectionRequestError::InvalidPassword
        })?
        .into();

    // Try load bank
    let bank = match BankStorage::try_load(&login_token.username) {
        Ok(bank_storage) => Bank::from(bank_storage),
        Err(_) => match BankStorage::create(&login_token.username) {
            Ok(bank_storage) => {
                log::info!("Created bank storage for account {}", &login_token.username);
                Bank::from(bank_storage)
            }
            Err(error) => {
                log::error!(
                    "Failed to create bank storage for account {} with error {}",
                    &login_token.username,
                    error
                );
                return Err(ConnectionRequestError::Failed);
            }
        },
    };

    // Try load character
    let character =
        CharacterStorage::try_load(&login_token.selected_character).map_err(|error| {
            log::error!(
                "Failed to load character {} with error {:?}",
                &login_token.selected_character,
                error
            );
            ConnectionRequestError::Failed
        })?;

    // Try find clan membership
    let mut clan_membership = ClanMembership(None);
    for (clan_entity, mut clan) in query_clans.iter_mut() {
        if let Some(clan_member) = clan.find_offline_member_mut(&character.info.name) {
            let &mut ClanMember::Offline {
                position,
                contribution,
                ..
            } = clan_member
            else {
                unreachable!();
            };

            *clan_member = ClanMember::Online {
                entity,
                position,
                contribution,
            };
            clan_membership = ClanMembership::new(clan_entity);
            break;
        }
    }

    // Update token
    login_token.game_client = Some(entity);
    game_client.login_token = login_token.token;

    // Associate world / game clients
    game_client.world_client_entity = login_token.world_client;
    world_client.game_client_entity = Some(entity);

    let status_effects = StatusEffects::new();
    let status_effects_regen = StatusEffectsRegen::new();

    let ability_values = game_data.ability_value_calculator.calculate(
        &character.info,
        &character.level,
        &character.equipment,
        &character.basic_stats,
        &character.skill_list,
        &status_effects,
    );

    // If the character was saved as dead, we must respawn them!
    let (health_points, mana_points, position) = if character.health_points.hp == 0 {
        (
            HealthPoints::new((3 * ability_values.get_max_health()) / 10),
            ManaPoints::new((3 * ability_values.get_max_mana()) / 10),
            Position::new(
                character.info.revive_position,
                character.info.revive_zone_id,
            ),
        )
    } else {
        (
            character.health_points,
            character.mana_points,
            character.position.clone(),
        )
    };

    let weapon_motion_type = game_data
        .items
        .get_equipped_weapon_item_data(&character.equipment, EquipmentIndex::Weapon)
        .map(|item_data| item_data.motion_type)
        .unwrap_or(0) as usize;

    let motion_data = MotionData::from_character(
        game_data.motions.as_ref(),
        weapon_motion_type,
        character.info.gender,
    );

    let move_mode = MoveMode::Run;
    let move_speed = MoveSpeed::new(ability_values.get_move_speed(&move_mode));

    online_friends.insert(character.info.unique_id, &character.info.name, entity, None);

    commands.entity(entity).insert((
        FriendList {
            friends: character.friends.clone(),
        },
        account,
        CharacterBundle {
            ability_values,
            basic_stats: character.basic_stats.clone(),
            bank,
            command: Command::default(),
            cooldowns: Cooldowns::default(),
            damage_sources: DamageSources::default_character(),
            equipment: character.equipment.clone(),
            experience_points: character.experience_points,
            health_points,
            hotbar: character.hotbar.clone(),
            info: character.info.clone(),
            inventory: character.inventory.clone(),
            level: character.level,
            mana_points,
            motion_data,
            move_mode,
            move_speed,
            next_command: NextCommand::default(),
            party_membership: PartyMembership::default(),
            passive_recovery_time: PassiveRecoveryTime::default(),
            position: position.clone(),
            quest_state: character.quest_state.clone(),
            recovery_rate_bonus: RecoveryRateBonus::default(),
            skill_list: character.skill_list.clone(),
            skill_points: character.skill_points,
            stamina: character.stamina,
            stat_points: character.stat_points,
            status_effects,
            status_effects_regen,
            summon_usage: Default::default(),
            team: Team::default_character(),
            union_membership: character.union_membership.clone(),
            clan_membership,
        },
    ));

    Ok((
        123,
        Box::new(CharacterData {
            character_info: character.info,
            position: position.position,
            zone_id: position.zone_id,
            basic_stats: character.basic_stats,
            level: character.level,
            equipment: character.equipment.clone(),
            experience_points: character.experience_points,
            skill_list: character.skill_list,
            hotbar: character.hotbar,
            health_points,
            mana_points,
            stat_points: character.stat_points,
            skill_points: character.skill_points,
            union_membership: character.union_membership,
            stamina: character.stamina,
        }),
        Box::new(CharacterDataItems {
            inventory: character.inventory,
            equipment: character.equipment,
        }),
        Box::new(character.quest_state),
    ))
}

pub fn game_server_authentication_system(
    mut commands: Commands,
    mut query: Query<(Entity, &mut GameClient), Without<CharacterInfo>>,
    mut query_world_client: Query<&mut WorldClient>,
    mut query_clans: Query<(Entity, &mut Clan)>,
    mut login_tokens: ResMut<LoginTokens>,
    mut online_friends: ResMut<OnlineFriends>,
    game_data: Res<GameData>,
) {
    query.for_each_mut(|(entity, mut game_client)| {
        if let Ok(message) = game_client.client_message_rx.try_recv() {
            match message {
                ClientMessage::GameConnectionRequest {
                    login_token,
                    password,
                } => {
                    match handle_game_connection_request(
                        &mut commands,
                        game_data.as_ref(),
                        login_tokens.as_mut(),
                        online_friends.as_mut(),
                        entity,
                        game_client.as_mut(),
                        login_token,
                        &password,
                        &mut query_world_client,
                        &mut query_clans,
                    ) {
                        Ok((
                            packet_sequence_id,
                            character_data,
                            character_data_items,
                            character_data_quest,
                        )) => {
                            game_client
                                .server_message_tx
                                .send(ServerMessage::ConnectionRequestSuccess {
                                    packet_sequence_id,
                                })
                                .ok();
                            game_client
                                .server_message_tx
                                .send(ServerMessage::CharacterData {
                                    data: character_data,
                                })
                                .ok();
                            game_client
                                .server_message_tx
                                .send(ServerMessage::CharacterDataItems {
                                    data: character_data_items,
                                })
                                .ok();
                            game_client
                                .server_message_tx
                                .send(ServerMessage::CharacterDataQuest {
                                    quest_state: character_data_quest,
                                })
                                .ok();
                        }
                        Err(error) => {
                            game_client
                                .server_message_tx
                                .send(ServerMessage::ConnectionRequestError { error })
                                .ok();
                        }
                    }
                }
                _ => warn!("Received unexpected client message {:?}", message),
            }
        }
    });
}

pub fn game_server_join_system(
    mut commands: Commands,
    mut query: Query<
        (
            Entity,
            &GameClient,
            &CharacterInfo,
            &ExperiencePoints,
            &mut Team,
            &HealthPoints,
            &ManaPoints,
            &Position,
            &FriendList,
        ),
        Without<ClientEntity>,
    >,
    mut client_entity_list: ResMut<ClientEntityList>,
    mut online_friends: ResMut<OnlineFriends>,
    game_data: Res<GameData>,
    mut quest_trigger_events: EventWriter<QuestTriggerEvent>,
    world_rates: Res<WorldRates>,
    world_time: Res<WorldTime>,
    zone_list: Res<crate::game::resources::ZoneList>,
    mut party_query: Query<(Entity, &mut Party)>,
    mut party_member_events: EventWriter<PartyMemberEvent>,
) {
    for (
        entity,
        game_client,
        character_info,
        experience_points,
        mut team,
        health_points,
        mana_points,
        position,
        friend_list,
    ) in query.iter_mut()
    {
        if let Ok(message) = game_client.client_message_rx.try_recv() {
            match message {
                ClientMessage::JoinZoneRequest => {
                    if let Ok(entity_id) = client_entity_join_zone(
                        &mut commands,
                        &mut client_entity_list,
                        entity,
                        ClientEntityType::Character,
                        position,
                    ) {
                        // See if we are in a party as an offline member
                        let mut reconnected_party_membership = None;
                        for (party_entity, mut party) in party_query.iter_mut() {
                            for party_member in party.members.iter_mut() {
                                if let PartyMember::Offline(
                                    party_member_character_id,
                                    party_member_name,
                                ) = party_member
                                {
                                    if *party_member_character_id == character_info.unique_id
                                        && party_member_name == &character_info.name
                                    {
                                        *party_member = PartyMember::Online(entity);
                                        reconnected_party_membership =
                                            Some(PartyMembership::new(party_entity));
                                        party_member_events.send(PartyMemberEvent::Reconnect {
                                            party_entity,
                                            reconnect_entity: entity,
                                            character_id: character_info.unique_id,
                                            name: character_info.name.clone(),
                                        });
                                        break;
                                    }
                                }
                            }
                        }

                        // Only overwrite PartyMembership if reconnecting
                        // from offline; otherwise keep existing membership
                        if let Some(party_membership) = reconnected_party_membership {
                            commands.entity(entity).insert(party_membership);
                        }

                        commands
                            .entity(entity)
                            .insert(ClientEntityVisibility::new())
                            .insert(PassiveRecoveryTime::default());

                        online_friends.update_client_entity_id(character_info.unique_id, entity_id);
                        *team = Team::default_character();

                        if let Some(zone_data) = game_data.zones.get_zone(position.zone_id) {
                            if let Some(join_trigger) = zone_data.join_trigger.as_ref() {
                                if should_fire_zone_join_trigger(position.zone_id) {
                                    quest_trigger_events.send(QuestTriggerEvent {
                                        trigger_entity: entity,
                                        trigger_hash: join_trigger.as_str().into(),
                                    });
                                }
                            }
                        }

                        let global_flags = game_data
                            .zones
                            .get_zone(position.zone_id)
                            .map(|zone_data| {
                                join_zone_global_flags(
                                    zone_data,
                                    zone_list.get_pvp_enabled(position.zone_id),
                                )
                            })
                            .unwrap_or(0);

                        game_client
                            .server_message_tx
                            .send(ServerMessage::JoinZone {
                                entity_id,
                                experience_points: *experience_points,
                                team: team.clone(),
                                global_flags,
                                health_points: *health_points,
                                mana_points: *mana_points,
                                world_ticks: world_time.ticks,
                                craft_rate: world_rates.craft_rate,
                                world_price_rate: world_rates.world_price_rate,
                                item_price_rate: world_rates.item_price_rate,
                                town_price_rate: world_rates.town_price_rate,
                            })
                            .ok();

                        game_client
                            .server_message_tx
                            .send(ServerMessage::FriendList {
                                friends: build_friend_infos(
                                    online_friends.as_ref(),
                                    &friend_list.friends,
                                ),
                            })
                            .ok();

                        let current_id = character_info.unique_id;
                        let current_friends = friend_list.friends.clone();
                        commands.add(move |world: &mut World| {
                            send_friend_status_to_online_friends(
                                world,
                                current_id,
                                &current_friends,
                                FriendStatus::Online,
                            );
                        });
                    }
                }
                _ => warn!("Received unexpected client message {:?}", message),
            }
        }
    }
}

#[derive(WorldQuery)]
#[world_query(mutable)]
pub struct GameClientQuery<'w> {
    entity: Entity,
    game_client: &'w GameClient,
    client_entity: &'w ClientEntity,
    client_entity_sector: &'w ClientEntitySector,
    position: &'w Position,
    ability_values: &'w AbilityValues,
    command: &'w Command,
    personal_store: Option<&'w PersonalStore>,
    dead: Option<&'w Dead>,
    level: &'w Level,
    move_speed: &'w MoveSpeed,
    team: &'w Team,
    friend_list: &'w FriendList,
    basic_stats: &'w mut BasicStats,
    character_info: &'w mut CharacterInfo,
    stat_points: &'w mut StatPoints,
    skill_points: &'w mut SkillPoints,
    skill_list: &'w mut SkillList,
    hotbar: &'w mut Hotbar,
    equipment: &'w mut Equipment,
    inventory: &'w mut Inventory,
    quest_state: &'w mut QuestState,
    move_mode: &'w mut MoveMode,
    mana_points: &'w mut ManaPoints,
}

#[derive(SystemParam)]
pub struct GameEvents<'w> {
    bank_events: EventWriter<'w, BankEvent>,
    chat_command_events: EventWriter<'w, ChatCommandEvent>,
    clan_events: EventWriter<'w, ClanEvent>,
    equipment_events: EventWriter<'w, EquipmentEvent>,
    item_life_events: EventWriter<'w, ItemLifeEvent>,
    npc_store_events: EventWriter<'w, NpcStoreEvent>,
    party_events: EventWriter<'w, PartyEvent>,
    personal_store_events: EventWriter<'w, PersonalStoreEvent>,
    quest_trigger_events: EventWriter<'w, QuestTriggerEvent>,
    revive_events: EventWriter<'w, ReviveEvent>,
    use_item_events: EventWriter<'w, UseItemEvent>,
}

pub fn game_server_main_system(
    mut commands: Commands,
    mut events: GameEvents,
    mut game_client_query: Query<GameClientQuery>,
    world_client_query: Query<&WorldClient>,
    mut client_entity_list: ResMut<ClientEntityList>,
    mut server_messages: ResMut<ServerMessages>,
    game_data: Res<GameData>,
    online_friends: Res<OnlineFriends>,
    world_rates: Res<WorldRates>,
    time: Res<Time>,
) {
    for mut game_client in game_client_query.iter_mut() {
        let mut entity_commands = commands.entity(game_client.entity);

        if let Ok(message) = game_client.game_client.client_message_rx.try_recv() {
            match message {
                ClientMessage::Chat { text } => {
                    if text.chars().next().map_or(false, |c| c == '/') {
                        events
                            .chat_command_events
                            .send(ChatCommandEvent::new(game_client.entity, text));
                    } else {
                        server_messages.send_entity_message(
                            game_client.client_entity,
                            ServerMessage::LocalChat {
                                entity_id: game_client.client_entity.id,
                                text,
                            },
                        );
                    }
                }
                ClientMessage::FriendListRequest => {
                    game_client
                        .game_client
                        .server_message_tx
                        .send(ServerMessage::FriendList {
                            friends: build_friend_infos(
                                online_friends.as_ref(),
                                &game_client.friend_list.friends,
                            ),
                        })
                        .ok();
                }
                ClientMessage::FriendAdd { name } => {
                    let sender_entity = game_client.entity;
                    let target_name = name.trim().to_string();
                    commands.add(move |world: &mut World| {
                        if target_name.is_empty() {
                            return;
                        }

                        let mut query = world.query::<(&CharacterInfo, &FriendList, &GameClient)>();
                        let Ok((sender_info, sender_friend_list, sender_game_client)) =
                            query.get(world, sender_entity)
                        else {
                            return;
                        };

                        if sender_info.name.eq_ignore_ascii_case(&target_name)
                            || contains_friend(&sender_friend_list.friends, sender_info.unique_id)
                        {
                            return;
                        }

                        if sender_friend_list
                            .friends
                            .iter()
                            .any(|friend| friend.name.eq_ignore_ascii_case(&target_name))
                        {
                            return;
                        }

                        let target_online = {
                            let online_friends = world.resource::<OnlineFriends>();
                            online_friends.get_by_name(&target_name)
                        };

                        let Some(target_online) = target_online else {
                            sender_game_client
                                .server_message_tx
                                .send(ServerMessage::FriendAddTargetNotFound {
                                    name: target_name.clone(),
                                })
                                .ok();
                            return;
                        };

                        if target_online.entity == sender_entity {
                            return;
                        }

                        if let Ok((_, _, target_game_client)) =
                            query.get(world, target_online.entity)
                        {
                            target_game_client
                                .server_message_tx
                                .send(ServerMessage::FriendAddRequest {
                                    requester_id: sender_info.unique_id,
                                    name: sender_info.name.clone(),
                                })
                                .ok();
                        }
                    });
                }
                ClientMessage::FriendAddResponse {
                    requester_id,
                    accept,
                } => {
                    let responder_entity = game_client.entity;
                    commands.add(move |world: &mut World| {
                        let requester_online = {
                            let online_friends = world.resource::<OnlineFriends>();
                            online_friends.get_by_id(requester_id)
                        };

                        let Some(requester_online) = requester_online else {
                            return;
                        };

                        let (
                            responder_id,
                            responder_name,
                            responder_server_message_tx,
                            requester_name,
                            requester_server_message_tx,
                        ) = {
                            let mut info_query = world.query::<(&CharacterInfo, &GameClient)>();
                            let Ok((responder_info, responder_game_client)) =
                                info_query.get(world, responder_entity)
                            else {
                                return;
                            };
                            let Ok((requester_info, requester_game_client)) =
                                info_query.get(world, requester_online.entity)
                            else {
                                return;
                            };

                            (
                                responder_info.unique_id,
                                responder_info.name.clone(),
                                responder_game_client.server_message_tx.clone(),
                                requester_info.name.clone(),
                                requester_game_client.server_message_tx.clone(),
                            )
                        };

                        if !accept {
                            requester_server_message_tx
                                .send(ServerMessage::FriendAddRejected {
                                    name: responder_name,
                                })
                                .ok();
                            return;
                        }

                        if let Some(mut responder_friend_list) =
                            world.get_mut::<FriendList>(responder_entity)
                        {
                            if !contains_friend(&responder_friend_list.friends, requester_id) {
                                responder_friend_list.friends.push(Friend {
                                    character_id: requester_id,
                                    name: requester_name.clone(),
                                });
                            }
                        }

                        if let Some(mut requester_friend_list) =
                            world.get_mut::<FriendList>(requester_online.entity)
                        {
                            if !contains_friend(&requester_friend_list.friends, responder_id) {
                                requester_friend_list.friends.push(Friend {
                                    character_id: responder_id,
                                    name: responder_name.clone(),
                                });
                            }
                        }

                        requester_server_message_tx
                            .send(ServerMessage::FriendAdded {
                                friend: FriendInfo {
                                    character_id: responder_id,
                                    name: responder_name.clone(),
                                    status: FriendStatus::Online,
                                },
                            })
                            .ok();
                        responder_server_message_tx
                            .send(ServerMessage::FriendAdded {
                                friend: FriendInfo {
                                    character_id: requester_id,
                                    name: requester_name,
                                    status: FriendStatus::Online,
                                },
                            })
                            .ok();
                    });
                }
                ClientMessage::FriendRemove { friend_id } => {
                    let sender_entity = game_client.entity;
                    commands.add(move |world: &mut World| {
                        let (sender_id, sender_name, removed_friend) = {
                            let mut query =
                                world.query::<(&CharacterInfo, &GameClient, &mut FriendList)>();
                            let Ok((sender_info, sender_game_client, mut sender_friend_list)) =
                                query.get_mut(world, sender_entity)
                            else {
                                return;
                            };

                            let removed_index = sender_friend_list
                                .friends
                                .iter()
                                .position(|friend| friend.character_id == friend_id);
                            let Some(removed_index) = removed_index else {
                                return;
                            };
                            let removed_friend = sender_friend_list.friends.remove(removed_index);

                            sender_game_client
                                .server_message_tx
                                .send(ServerMessage::FriendRemoved { friend_id })
                                .ok();

                            (
                                sender_info.unique_id,
                                sender_info.name.clone(),
                                removed_friend,
                            )
                        };

                        let target_online = {
                            let online_friends = world.resource::<OnlineFriends>();
                            online_friends.get_by_id(friend_id)
                        };

                        if let Some(target_online) = target_online {
                            if let Some(mut target_friend_list) =
                                world.get_mut::<FriendList>(target_online.entity)
                            {
                                if let Some(remove_index) = target_friend_list
                                    .friends
                                    .iter()
                                    .position(|friend| friend.character_id == sender_id)
                                {
                                    target_friend_list.friends.remove(remove_index);
                                }
                            }

                            if let Some(target_game_client) =
                                world.get::<GameClient>(target_online.entity)
                            {
                                target_game_client
                                    .server_message_tx
                                    .send(ServerMessage::FriendStatusChanged {
                                        friend_id: sender_id,
                                        status: FriendStatus::Deleted,
                                    })
                                    .ok();
                            }
                        } else if let Ok(mut target_character) =
                            CharacterStorage::try_load(&removed_friend.name)
                        {
                            if let Some(remove_index) = target_character
                                .friends
                                .iter()
                                .position(|friend| friend.character_id == sender_id)
                            {
                                target_character.friends.remove(remove_index);
                                target_character.save().ok();
                            }
                        }

                        let _ = sender_name;
                    });
                }
                ClientMessage::FriendChat { friend_id, text } => {
                    let sender_entity = game_client.entity;
                    commands.add(move |world: &mut World| {
                        if text.is_empty() {
                            return;
                        }

                        let (sender_id, sender_name, sender_has_friend) = {
                            let mut sender_query = world.query::<(&CharacterInfo, &FriendList)>();
                            let Ok((sender_info, sender_friend_list)) =
                                sender_query.get(world, sender_entity)
                            else {
                                return;
                            };

                            (
                                sender_info.unique_id,
                                sender_info.name.clone(),
                                contains_friend(&sender_friend_list.friends, friend_id),
                            )
                        };

                        if !sender_has_friend {
                            return;
                        }

                        let target_online = {
                            let online_friends = world.resource::<OnlineFriends>();
                            online_friends.get_by_id(friend_id)
                        };
                        let Some(target_online) = target_online else {
                            return;
                        };

                        let mut target_query = world.query::<(&FriendList, &GameClient)>();
                        let Ok((target_friend_list, target_game_client)) =
                            target_query.get(world, target_online.entity)
                        else {
                            return;
                        };

                        if !contains_friend(&target_friend_list.friends, sender_id) {
                            return;
                        }

                        target_game_client
                            .server_message_tx
                            .send(ServerMessage::FriendChat {
                                friend_id: sender_id,
                                from_name: sender_name,
                                text,
                            })
                            .ok();
                    });
                }
                ClientMessage::Move {
                    target_entity_id,
                    x,
                    y,
                    z,
                } => {
                    if game_client.personal_store.is_some() {
                        continue;
                    }

                    let mut move_target_entity = None;
                    if let Some(target_entity_id) = target_entity_id {
                        if let Some((target_entity, _, _)) = client_entity_list
                            .get_zone(game_client.position.zone_id)
                            .and_then(|zone| zone.get_entity(target_entity_id))
                        {
                            move_target_entity = Some(*target_entity);
                        }
                    }

                    let destination = Vec3::new(x, y, z as f32);
                    entity_commands.insert(NextCommand::with_move(
                        destination,
                        move_target_entity,
                        None,
                    ));
                }
                ClientMessage::Attack { target_entity_id } => {
                    if let Some((target_entity, _, _)) = client_entity_list
                        .get_zone(game_client.position.zone_id)
                        .and_then(|zone| zone.get_entity(target_entity_id))
                    {
                        entity_commands.insert(NextCommand::with_attack(*target_entity));
                    } else {
                        entity_commands.insert(NextCommand::with_stop(true));
                    }
                }
                ClientMessage::SetHotbarSlot { slot_index, slot } => {
                    if game_client
                        .hotbar
                        .set_slot(slot_index, slot.clone())
                        .is_some()
                    {
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::SetHotbarSlot { slot_index, slot })
                            .ok();
                    }
                }
                ClientMessage::ChangeEquipment {
                    equipment_index,
                    item_slot,
                } => {
                    events
                        .equipment_events
                        .send(EquipmentEvent::ChangeEquipment {
                            entity: game_client.entity,
                            equipment_index,
                            item_slot,
                        });
                }
                ClientMessage::ChangeVehiclePart {
                    vehicle_part_index,
                    item_slot,
                } => {
                    events
                        .equipment_events
                        .send(EquipmentEvent::ChangeVehiclePart {
                            entity: game_client.entity,
                            vehicle_part_index,
                            item_slot,
                        });
                }
                ClientMessage::ChangeAmmo {
                    ammo_index,
                    item_slot,
                } => {
                    events.equipment_events.send(EquipmentEvent::ChangeAmmo {
                        entity: game_client.entity,
                        ammo_index,
                        item_slot,
                    });
                }
                ClientMessage::IncreaseBasicStat { basic_stat_type } => {
                    if let Some(cost) = game_data
                        .ability_value_calculator
                        .calculate_basic_stat_increase_cost(
                            &game_client.basic_stats,
                            basic_stat_type,
                        )
                    {
                        if cost < game_client.stat_points.points {
                            let value = match basic_stat_type {
                                BasicStatType::Strength => &mut game_client.basic_stats.strength,
                                BasicStatType::Dexterity => &mut game_client.basic_stats.dexterity,
                                BasicStatType::Intelligence => {
                                    &mut game_client.basic_stats.intelligence
                                }
                                BasicStatType::Concentration => {
                                    &mut game_client.basic_stats.concentration
                                }
                                BasicStatType::Charm => &mut game_client.basic_stats.charm,
                                BasicStatType::Sense => &mut game_client.basic_stats.sense,
                            };

                            game_client.stat_points.points -= cost;
                            *value += 1;

                            game_client
                                .game_client
                                .server_message_tx
                                .send(ServerMessage::UpdateBasicStat {
                                    basic_stat_type,
                                    value: *value,
                                })
                                .ok();
                        }
                    }
                }
                ClientMessage::PickupItemDrop { target_entity_id } => {
                    if let Some((target_entity, _, _)) = client_entity_list
                        .get_zone(game_client.position.zone_id)
                        .and_then(|zone| zone.get_entity(target_entity_id))
                    {
                        entity_commands.insert(NextCommand::with_pickup_item_drop(*target_entity));
                    } else {
                        entity_commands.insert(NextCommand::with_stop(true));
                    }
                }
                ClientMessage::Logout | ClientMessage::ReturnToCharacterSelect => {
                    if let ClientMessage::ReturnToCharacterSelect = message {
                        // Send ReturnToCharacterSelect via world_client
                        world_client_query.for_each(|world_client| {
                            if world_client.login_token == game_client.game_client.login_token {
                                world_client
                                    .server_message_tx
                                    .send(ServerMessage::ReturnToCharacterSelect)
                                    .ok();
                            }
                        });
                    }

                    game_client
                        .game_client
                        .server_message_tx
                        .send(ServerMessage::LogoutSuccess)
                        .ok();

                    client_entity_leave_zone(
                        &mut commands,
                        &mut client_entity_list,
                        game_client.entity,
                        game_client.client_entity,
                        game_client.client_entity_sector,
                        game_client.position,
                    );
                }
                ClientMessage::ReviveCurrentZone => {
                    if game_client.dead.is_some() {
                        events.revive_events.send(ReviveEvent {
                            entity: game_client.entity,
                            position: RevivePosition::CurrentZone,
                        });
                    }
                }
                ClientMessage::ReviveSaveZone => {
                    if game_client.dead.is_some() {
                        events.revive_events.send(ReviveEvent {
                            entity: game_client.entity,
                            position: RevivePosition::SaveZone,
                        });
                    }
                }
                ClientMessage::SetReviveSaveZone => {
                    if let Some(zone_data) = game_data.zones.get_zone(game_client.position.zone_id)
                    {
                        let revive_position = zone_data
                            .get_closest_revive_position(zone_data.start_position)
                            .unwrap_or(zone_data.start_position);
                        game_client.character_info.revive_zone_id = game_client.position.zone_id;
                        game_client.character_info.revive_position = revive_position;
                    }
                }
                ClientMessage::QuestDelete { slot, quest_id } => {
                    if let Some(quest_slot) = game_client.quest_state.get_quest_slot_mut(slot) {
                        if let Some(quest) = quest_slot {
                            if quest.quest_id == quest_id {
                                *quest_slot = None;
                                game_client
                                    .game_client
                                    .server_message_tx
                                    .send(ServerMessage::QuestDeleteResult {
                                        success: true,
                                        slot,
                                        quest_id,
                                    })
                                    .ok();
                            }
                        }
                    }
                }
                ClientMessage::QuestTrigger { trigger } => {
                    events.quest_trigger_events.send(QuestTriggerEvent {
                        trigger_entity: game_client.entity,
                        trigger_hash: trigger,
                    });
                }
                ClientMessage::PersonalStoreListItems { store_entity_id } => {
                    if let Some((store_entity, _, _)) = client_entity_list
                        .get_zone(game_client.position.zone_id)
                        .and_then(|zone| zone.get_entity(store_entity_id))
                    {
                        events
                            .personal_store_events
                            .send(PersonalStoreEvent::ListItems {
                                store_entity: *store_entity,
                                list_entity: game_client.entity,
                            });
                    }
                }
                ClientMessage::PersonalStoreBuyItem {
                    store_entity_id,
                    store_slot_index,
                    buy_item,
                } => {
                    if let Some((store_entity, _, _)) = client_entity_list
                        .get_zone(game_client.position.zone_id)
                        .and_then(|zone| zone.get_entity(store_entity_id))
                    {
                        events
                            .personal_store_events
                            .send(PersonalStoreEvent::BuyItem {
                                store_entity: *store_entity,
                                buyer_entity: game_client.entity,
                                store_slot_index,
                                buy_item,
                            });
                    }
                }
                ClientMessage::UseItem {
                    item_slot,
                    target_entity_id,
                } => {
                    let target_entity = target_entity_id
                        .and_then(|target_entity_id| {
                            client_entity_list
                                .get_zone(game_client.position.zone_id)
                                .and_then(|zone| zone.get_entity(target_entity_id))
                        })
                        .map(|(target_entity, _, _)| *target_entity);

                    events.use_item_events.send(UseItemEvent::from_inventory(
                        game_client.entity,
                        item_slot,
                        target_entity,
                    ));
                }
                ClientMessage::LevelUpSkill { skill_slot } => {
                    skill_list_try_level_up_skill(
                        &game_data,
                        &mut SkillListBundle {
                            skill_list: &mut game_client.skill_list,
                            skill_points: Some(&mut game_client.skill_points),
                            game_client: Some(game_client.game_client),
                            ability_values: game_client.ability_values,
                            level: game_client.level,
                            move_speed: Some(game_client.move_speed),
                            team: Some(game_client.team),
                            character_info: Some(&game_client.character_info),
                            experience_points: None,
                            inventory: Some(&game_client.inventory),
                            stamina: None,
                            stat_points: None,
                            union_membership: None,
                            health_points: None,
                            mana_points: None,
                        },
                        skill_slot,
                    )
                    .ok();
                }
                ClientMessage::CastSkillSelf { skill_slot } => {
                    if let Some(skill) = game_client.skill_list.get_skill(skill_slot) {
                        entity_commands
                            .insert(NextCommand::with_cast_skill_target_self(skill, None));
                    }
                }
                ClientMessage::CastSkillTargetEntity {
                    skill_slot,
                    target_entity_id,
                } => {
                    if let Some(skill) = game_client.skill_list.get_skill(skill_slot) {
                        if let Some((target_entity, _, _)) = client_entity_list
                            .get_zone(game_client.position.zone_id)
                            .and_then(|zone| zone.get_entity(target_entity_id))
                        {
                            entity_commands.insert(NextCommand::with_cast_skill_target_entity(
                                skill,
                                *target_entity,
                                None,
                            ));
                        }
                    }
                }
                ClientMessage::CastSkillTargetPosition {
                    skill_slot,
                    position,
                } => {
                    if let Some(skill) = game_client.skill_list.get_skill(skill_slot) {
                        entity_commands.insert(NextCommand::with_cast_skill_target_position(
                            skill, position,
                        ));
                    }
                }
                ClientMessage::NpcStoreTransaction {
                    npc_entity_id,
                    buy_items,
                    sell_items,
                } => {
                    if let Some((npc_entity, _, _)) = client_entity_list
                        .get_zone(game_client.position.zone_id)
                        .and_then(|zone| zone.get_entity(npc_entity_id))
                    {
                        events.npc_store_events.send(NpcStoreEvent {
                            store_entity: *npc_entity,
                            transaction_entity: game_client.entity,
                            buy_items,
                            sell_items,
                        });
                    }
                }
                ClientMessage::SitToggle => {
                    if matches!(game_client.command.command, CommandData::Sit) {
                        entity_commands.insert(NextCommand::with_standing());
                    } else {
                        entity_commands.insert(NextCommand::with_sitting());
                    }
                }
                ClientMessage::RunToggle => {
                    if match *game_client.move_mode {
                        MoveMode::Walk => {
                            *game_client.move_mode = MoveMode::Run;
                            true
                        }
                        MoveMode::Run => {
                            *game_client.move_mode = MoveMode::Walk;
                            true
                        }
                        MoveMode::Drive => false,
                    } {
                        server_messages.send_entity_message(
                            game_client.client_entity,
                            ServerMessage::MoveToggle {
                                entity_id: game_client.client_entity.id,
                                move_mode: *game_client.move_mode,
                                run_speed: None,
                            },
                        );
                    }
                }
                ClientMessage::DriveToggle => {
                    if match *game_client.move_mode {
                        MoveMode::Walk | MoveMode::Run => {
                            // Must have body, engine, and leg parts equipped to drive
                            let has_body = game_client
                                .equipment
                                .get_vehicle_item(VehiclePartIndex::Body)
                                .is_some();
                            let has_engine = game_client
                                .equipment
                                .get_vehicle_item(VehiclePartIndex::Engine)
                                .is_some();
                            let has_leg = game_client
                                .equipment
                                .get_vehicle_item(VehiclePartIndex::Leg)
                                .is_some();

                            if !has_body || !has_engine || !has_leg {
                                false
                            } else {
                                // Starting driving decreases vehicle engine life
                                events.item_life_events.send(
                                    ItemLifeEvent::DecreaseVehicleEngineLife {
                                        entity: game_client.entity,
                                        amount: None,
                                    },
                                );

                                // Start driving
                                *game_client.move_mode = MoveMode::Drive;
                                commands
                                    .entity(game_client.entity)
                                    .insert(DrivingTime::default());

                                true
                            }
                        }
                        MoveMode::Drive => {
                            *game_client.move_mode = MoveMode::Run;
                            commands.entity(game_client.entity).remove::<DrivingTime>();
                            true
                        }
                    } {
                        server_messages.send_entity_message(
                            game_client.client_entity,
                            ServerMessage::MoveToggle {
                                entity_id: game_client.client_entity.id,
                                move_mode: *game_client.move_mode,
                                run_speed: None,
                            },
                        );
                    }
                }
                ClientMessage::DropMoney { quantity } => {
                    let mut money = Money(quantity as i64);
                    if money > game_client.inventory.money {
                        money = game_client.inventory.money;
                        game_client.inventory.money = Money(0)
                    } else {
                        game_client.inventory.money = game_client.inventory.money - money;
                    }

                    if money > Money(0) {
                        ItemDropBundle::spawn(
                            &mut commands,
                            &mut client_entity_list,
                            DroppedItem::Money(money),
                            game_client.position,
                            None,
                            None,
                            &time,
                        );

                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::UpdateMoney {
                                money: game_client.inventory.money,
                            })
                            .ok();
                    }
                }
                ClientMessage::DropItem {
                    item_slot,
                    quantity,
                } => {
                    if let Some(inventory_slot) = game_client.inventory.get_item_slot_mut(item_slot)
                    {
                        let quantity = u32::min(
                            quantity as u32,
                            inventory_slot
                                .as_ref()
                                .map(|item| item.get_quantity())
                                .unwrap_or(0),
                        );
                        let item = inventory_slot.try_take_quantity(quantity);

                        if let Some(item) = item {
                            ItemDropBundle::spawn(
                                &mut commands,
                                &mut client_entity_list,
                                DroppedItem::Item(item),
                                game_client.position,
                                None,
                                None,
                                &time,
                            );

                            game_client
                                .game_client
                                .server_message_tx
                                .send(ServerMessage::UpdateInventory {
                                    items: vec![(item_slot, inventory_slot.clone())],
                                    money: None,
                                })
                                .ok();
                        }
                    }
                }
                ClientMessage::UseEmote { motion_id, is_stop } => {
                    entity_commands.insert(NextCommand::with_emote(motion_id, is_stop));
                }
                ClientMessage::WarpGateRequest { warp_gate_id } => {
                    if let Some(warp_gate) = game_data.warp_gates.get_warp_gate(warp_gate_id) {
                        if let Some(zone) = game_data.zones.get_zone(warp_gate.target_zone) {
                            if let Some(event_position) =
                                zone.event_positions.get(&warp_gate.target_event_object)
                            {
                                client_entity_teleport_zone(
                                    &mut commands,
                                    &mut client_entity_list,
                                    game_client.entity,
                                    game_client.client_entity,
                                    game_client.client_entity_sector,
                                    game_client.position,
                                    Position::new(*event_position, warp_gate.target_zone),
                                    Some(game_client.game_client),
                                );
                            }
                        }
                    }
                }
                ClientMessage::PartyCreate { invited_entity_id }
                | ClientMessage::PartyInvite { invited_entity_id } => {
                    if let Some(&(invited_entity, _, _)) = client_entity_list
                        .get_zone(game_client.position.zone_id)
                        .and_then(|zone| zone.get_entity(invited_entity_id))
                    {
                        events.party_events.send(PartyEvent::Invite {
                            owner_entity: game_client.entity,
                            invited_entity,
                        });
                    }
                }
                ClientMessage::PartyLeave => {
                    events.party_events.send(PartyEvent::Leave {
                        leaver_entity: game_client.entity,
                    });
                }
                ClientMessage::PartyChangeOwner {
                    new_owner_entity_id,
                } => {
                    if let Some(&(new_owner_entity, _, _)) = client_entity_list
                        .get_zone(game_client.position.zone_id)
                        .and_then(|zone| zone.get_entity(new_owner_entity_id))
                    {
                        events.party_events.send(PartyEvent::ChangeOwner {
                            owner_entity: game_client.entity,
                            new_owner_entity,
                        });
                    }
                }
                ClientMessage::PartyKick { character_id } => {
                    events.party_events.send(PartyEvent::Kick {
                        owner_entity: game_client.entity,
                        kick_character_id: character_id,
                    });
                }
                ClientMessage::PartyAcceptCreateInvite { owner_entity_id }
                | ClientMessage::PartyAcceptJoinInvite { owner_entity_id } => {
                    if let Some(&(owner_entity, _, _)) = client_entity_list
                        .get_zone(game_client.position.zone_id)
                        .and_then(|zone| zone.get_entity(owner_entity_id))
                    {
                        events.party_events.send(PartyEvent::AcceptInvite {
                            owner_entity,
                            invited_entity: game_client.entity,
                        });
                    }
                }
                ClientMessage::PartyRejectInvite {
                    reason,
                    owner_entity_id,
                } => {
                    if let Some(&(owner_entity, _, _)) = client_entity_list
                        .get_zone(game_client.position.zone_id)
                        .and_then(|zone| zone.get_entity(owner_entity_id))
                    {
                        events.party_events.send(PartyEvent::RejectInvite {
                            reason,
                            owner_entity,
                            invited_entity: game_client.entity,
                        });
                    }
                }
                ClientMessage::PartyUpdateRules {
                    item_sharing,
                    xp_sharing,
                } => {
                    events.party_events.send(PartyEvent::UpdateRules {
                        owner_entity: game_client.entity,
                        item_sharing,
                        xp_sharing,
                    });
                }
                ClientMessage::MoveCollision { position } => {
                    if game_client.personal_store.is_some() {
                        continue;
                    }

                    // TODO: Sanity check position
                    entity_commands
                        .insert(NextCommand::with_move(position, None, None))
                        .insert(Position::new(position, game_client.position.zone_id));
                }
                ClientMessage::CraftInsertGem {
                    equipment_index,
                    item_slot,
                } => {
                    if game_client
                        .inventory
                        .get_item(item_slot)
                        .and_then(|item| {
                            if !matches!(item.get_item_type(), ItemType::Gem) {
                                None
                            } else {
                                game_data.items.get_base_item(item.get_item_reference())
                            }
                        })
                        .map_or(false, |item_data| item_data.class == ItemClass::Jewel)
                    {
                        if let Some(equipment_item) =
                            game_client.equipment.get_equipment_item(equipment_index)
                        {
                            if !equipment_item.has_socket {
                                game_client
                                    .game_client
                                    .server_message_tx
                                    .send(ServerMessage::CraftInsertGemError {
                                        error: CraftInsertGemError::NoSocket,
                                    })
                                    .ok();
                            } else if equipment_item.gem > 300 {
                                game_client
                                    .game_client
                                    .server_message_tx
                                    .send(ServerMessage::CraftInsertGemError {
                                        error: CraftInsertGemError::SocketFull,
                                    })
                                    .ok();
                            } else {
                                let equipment_item = game_client
                                    .equipment
                                    .get_equipment_slot_mut(equipment_index)
                                    .as_mut()
                                    .unwrap();

                                if let Some(gem_item) = game_client
                                    .inventory
                                    .get_item_slot_mut(item_slot)
                                    .unwrap()
                                    .try_take_quantity(1)
                                {
                                    equipment_item.gem = gem_item.get_item_number() as u16;

                                    game_client
                                        .game_client
                                        .server_message_tx
                                        .send(ServerMessage::CraftInsertGem {
                                            update_items: vec![
                                                (
                                                    item_slot,
                                                    game_client
                                                        .inventory
                                                        .get_item(item_slot)
                                                        .cloned(),
                                                ),
                                                (
                                                    ItemSlot::Equipment(equipment_index),
                                                    game_client
                                                        .equipment
                                                        .get_equipment_item(equipment_index)
                                                        .cloned()
                                                        .map(Item::Equipment),
                                                ),
                                            ],
                                        })
                                        .ok();

                                    server_messages.send_entity_message(
                                        game_client.client_entity,
                                        ServerMessage::UpdateEquipment {
                                            entity_id: game_client.client_entity.id,
                                            equipment_index,
                                            item: game_client
                                                .equipment
                                                .get_equipment_item(equipment_index)
                                                .cloned(),
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
                ClientMessage::CraftCreateItem {
                    skill_slot,
                    target_item_type,
                    target_item_number,
                    material_inventory_slots,
                } => {
                    // Validate the skill
                    let skill_id = game_client.skill_list.get_skill(skill_slot);
                    let skill_data = skill_id.and_then(|id| game_data.skills.get_skill(id));

                    if skill_data.is_none() {
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::CraftCreateItemError {
                                error: CraftCreateItemError::InvalidCondition,
                            })
                            .ok();
                        continue;
                    }
                    let skill_data = skill_data.unwrap();

                    // Get target item data
                    let target_item_ref = ItemReference::new(target_item_type, target_item_number);
                    let target_item_data = game_data.items.get_base_item(target_item_ref);

                    if target_item_data.is_none() {
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::CraftCreateItemError {
                                error: CraftCreateItemError::InvalidItem,
                            })
                            .ok();
                        continue;
                    }
                    let target_item_data = target_item_data.unwrap();

                    // Check skill's item_make_number matches item's craft_skill_type
                    if skill_data.item_make_number != target_item_data.craft_skill_type {
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::CraftCreateItemError {
                                error: CraftCreateItemError::InvalidItem,
                            })
                            .ok();
                        continue;
                    }

                    // Check skill level >= item's craft_skill_level
                    if skill_data.level < target_item_data.craft_skill_level {
                        warn!(
                            "CraftCreateItem NeedSkillLevel: slot {:?}, skill {:?}, current {}, target {:?} #{}, required {}",
                            skill_slot,
                            skill_id,
                            skill_data.level,
                            target_item_ref.item_type,
                            target_item_ref.item_number,
                            target_item_data.craft_skill_level
                        );
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::CraftCreateItemError {
                                error: CraftCreateItemError::NeedSkillLevel,
                            })
                            .ok();
                        continue;
                    }
                    let required_mp = manufacture_required_mp(skill_data);
                    if game_client.mana_points.mp < required_mp {
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::CraftCreateItemError {
                                error: CraftCreateItemError::InvalidCondition,
                            })
                            .ok();
                        continue;
                    }

                    // Look up the product recipe
                    let product = game_data
                        .products
                        .get_product(target_item_data.craft_material);

                    if product.is_none() {
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::CraftCreateItemError {
                                error: CraftCreateItemError::InvalidItem,
                            })
                            .ok();
                        continue;
                    }
                    let product = product.unwrap();

                    // Validate materials exist in inventory
                    let mut materials_valid = true;
                    for (i, required_mat) in product.materials.iter().enumerate() {
                        let requirement =
                            resolve_craft_material_requirement(&game_data, required_mat.item);
                        if let Some(inv_item) =
                            game_client.inventory.get_item(material_inventory_slots[i])
                        {
                            let has_enough = match (inv_item, &requirement) {
                                (
                                    Item::Stackable(s),
                                    ResolvedCraftMaterialRequirement::Item(item),
                                ) => s.item == *item && s.quantity >= required_mat.quantity,
                                (
                                    Item::Stackable(s),
                                    ResolvedCraftMaterialRequirement::ItemClass(required_class),
                                ) => {
                                    s.quantity >= required_mat.quantity
                                        && game_data
                                            .items
                                            .get_base_item(s.item)
                                            .map_or(false, |item_data| {
                                                item_data.class == *required_class
                                            })
                                }
                                (_, ResolvedCraftMaterialRequirement::Unknown) => {
                                    warn!(
                                        "Unresolved craft material requirement for target {:?} #{}: required {:?} #{}",
                                        target_item_ref.item_type,
                                        target_item_ref.item_number,
                                        required_mat.item.item_type,
                                        required_mat.item.item_number
                                    );
                                    false
                                }
                                _ => false,
                            };
                            if !has_enough {
                                materials_valid = false;
                                break;
                            }
                        } else {
                            materials_valid = false;
                            break;
                        }
                    }

                    if !materials_valid {
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::CraftCreateItemError {
                                error: CraftCreateItemError::NeedItem,
                            })
                            .ok();
                        continue;
                    }

                    let success_chance = manufacture_success_chance(
                        skill_data.level,
                        target_item_data.craft_skill_level,
                        world_rates.craft_rate,
                    );
                    let roll = (rand::random::<u32>() % 100) as i32;
                    let success = roll < success_chance;

                    let crafted_item = if target_item_type.is_equipment_item() {
                        let mut equip =
                            EquipmentItem::new(target_item_ref, target_item_data.durability);
                        if let Some(ref mut equip) = equip {
                            equip.is_crafted = true;
                        }
                        equip.map(Item::Equipment)
                    } else {
                        Some(Item::Stackable(StackableItem {
                            item: target_item_ref,
                            quantity: 1,
                        }))
                    };

                    let Some(crafted_item) = crafted_item else {
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::CraftCreateItemError {
                                error: CraftCreateItemError::InvalidItem,
                            })
                            .ok();
                        continue;
                    };

                    // Hardening: if this attempt would succeed, pre-validate output insertion
                    // before consuming materials so a full inventory doesn't burn ingredients.
                    if success {
                        let mut inventory_after_success = game_client.inventory.clone();
                        for (i, required_mat) in product.materials.iter().enumerate() {
                            inventory_after_success.try_take_quantity(
                                material_inventory_slots[i],
                                required_mat.quantity,
                            );
                        }

                        if inventory_after_success
                            .try_add_item(crafted_item.clone())
                            .is_err()
                        {
                            warn!(
                                "CraftCreateItem blocked: inventory full for target {:?} #{}",
                                target_item_ref.item_type, target_item_ref.item_number
                            );
                            game_client
                                .game_client
                                .server_message_tx
                                .send(ServerMessage::CraftCreateItemError {
                                    error: CraftCreateItemError::InvalidCondition,
                                })
                                .ok();
                            continue;
                        }
                    }

                    // Consume materials
                    for (i, required_mat) in product.materials.iter().enumerate() {
                        game_client
                            .inventory
                            .try_take_quantity(material_inventory_slots[i], required_mat.quantity);
                    }
                    let material_updates = collect_craft_material_inventory_updates(
                        &game_client.inventory,
                        &material_inventory_slots,
                    );
                    game_client.mana_points.mp = (game_client.mana_points.mp - required_mp).max(0);
                    let updated_mp = game_client.mana_points.mp;

                    if success {
                        match game_client.inventory.try_add_item(crafted_item) {
                            Ok((slot, _)) => {
                                game_client
                                    .game_client
                                    .server_message_tx
                                    .send(ServerMessage::UpdateInventory {
                                        items: material_updates,
                                        money: None,
                                    })
                                    .ok();
                                game_client
                                    .game_client
                                    .server_message_tx
                                    .send(ServerMessage::UpdateAbilityValueSet {
                                        ability_type: AbilityType::Mana,
                                        value: updated_mp,
                                    })
                                    .ok();

                                game_client
                                    .game_client
                                    .server_message_tx
                                    .send(ServerMessage::CraftCreateItemSuccess {
                                        inventory_slot: slot,
                                        item: game_client
                                            .inventory
                                            .get_item(slot)
                                            .cloned()
                                            .unwrap(),
                                    })
                                    .ok();
                            }
                            Err(_) => {
                                warn!(
                                    "CraftCreateItem add-item failed after precheck for target {:?} #{}",
                                    target_item_ref.item_type,
                                    target_item_ref.item_number
                                );
                                game_client
                                    .game_client
                                    .server_message_tx
                                    .send(ServerMessage::UpdateInventory {
                                        items: material_updates,
                                        money: None,
                                    })
                                    .ok();
                                game_client
                                    .game_client
                                    .server_message_tx
                                    .send(ServerMessage::UpdateAbilityValueSet {
                                        ability_type: AbilityType::Mana,
                                        value: updated_mp,
                                    })
                                    .ok();
                                game_client
                                    .game_client
                                    .server_message_tx
                                    .send(ServerMessage::CraftCreateItemError {
                                        error: CraftCreateItemError::InvalidCondition,
                                    })
                                    .ok();
                            }
                        }
                    } else {
                        // Crafting failed by RNG - materials are consumed.
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::UpdateInventory {
                                items: material_updates,
                                money: None,
                            })
                            .ok();
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::UpdateAbilityValueSet {
                                ability_type: AbilityType::Mana,
                                value: updated_mp,
                            })
                            .ok();

                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::CraftCreateItemError {
                                error: CraftCreateItemError::Failed,
                            })
                            .ok();
                    }
                }
                ClientMessage::CraftSkillUpgradeItem {
                    skill_slot,
                    item_slot,
                    ingredients,
                } => {
                    // Validate skill
                    let skill_id = game_client.skill_list.get_skill(skill_slot);
                    let skill_data = skill_id.and_then(|id| game_data.skills.get_skill(id));

                    if skill_data.is_none() {
                        continue;
                    }
                    let skill_data = skill_data.unwrap();
                    if skill_data.skill_type != SkillType::CreateWindow
                        || skill_data.item_make_number != 42
                    {
                        warn!(
                            "CraftSkillUpgradeItem invalid skill for upgrade: {:?} make_number {}",
                            skill_data.skill_type, skill_data.item_make_number
                        );
                        continue;
                    }
                    let required_mp = manufacture_required_mp(skill_data);

                    // Get the target equipment item
                    let target_item = game_client.inventory.get_item(item_slot).cloned();
                    let target_equip = match &target_item {
                        Some(Item::Equipment(equip)) => Some(equip.clone()),
                        _ => None,
                    };

                    if target_equip.is_none() {
                        continue;
                    }
                    let target_equip = target_equip.unwrap();

                    if target_equip.grade >= 9 {
                        continue;
                    }

                    let required_quantities = match validate_upgrade_materials(
                        &game_data,
                        &game_client.inventory,
                        target_equip.item,
                        target_equip.grade,
                        &ingredients,
                    ) {
                        Ok(required_quantities) => required_quantities,
                        Err(reason) => {
                            warn!(
                                "CraftSkillUpgradeItem invalid materials for target {:?} #{} grade {}: {}",
                                target_equip.item.item_type,
                                target_equip.item.item_number,
                                target_equip.grade,
                                reason
                            );
                            continue;
                        }
                    };

                    if game_client.mana_points.mp < required_mp {
                        warn!(
                            "CraftSkillUpgradeItem insufficient MP: have {}, need {}",
                            game_client.mana_points.mp, required_mp
                        );
                        continue;
                    }

                    // Consume exact required ingredient quantities
                    for (ingredient_slot, required_quantity) in
                        ingredients.iter().zip(required_quantities.iter())
                    {
                        if *required_quantity == 0 {
                            continue;
                        }
                        game_client
                            .inventory
                            .try_take_quantity(*ingredient_slot, *required_quantity);
                    }
                    let material_updates = collect_craft_material_inventory_updates(
                        &game_client.inventory,
                        &ingredients,
                    );

                    game_client.mana_points.mp = (game_client.mana_points.mp - required_mp).max(0);
                    let updated_mp = game_client.mana_points.mp;
                    game_client
                        .game_client
                        .server_message_tx
                        .send(ServerMessage::UpdateAbilityValueSet {
                            ability_type: AbilityType::Mana,
                            value: updated_mp,
                        })
                        .ok();

                    // Simplified upgrade formula: 90 - grade*8 success rate
                    let success_chance = (90i32 - target_equip.grade as i32 * 8).clamp(10, 95);
                    let roll = rand::random::<i32>().unsigned_abs() as i32 % 100;
                    let success = roll < success_chance;

                    if success {
                        // Upgrade: increase grade
                        if let Some(slot_ref) = game_client.inventory.get_item_slot_mut(item_slot) {
                            if let Some(Item::Equipment(ref mut equip)) = slot_ref {
                                equip.grade += 1;
                            }
                        }
                        let mut update_items = Vec::new();
                        update_items.push((
                            item_slot,
                            game_client.inventory.get_item(item_slot).cloned(),
                        ));
                        update_items.extend(material_updates.iter().cloned());
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::CraftUpgradeSuccess { update_items })
                            .ok();
                    } else {
                        // Failed: decrease grade by 1 (min 0)
                        if let Some(slot_ref) = game_client.inventory.get_item_slot_mut(item_slot) {
                            if let Some(Item::Equipment(ref mut equip)) = slot_ref {
                                equip.grade = equip.grade.saturating_sub(1);
                            }
                        }
                        let mut update_items = Vec::new();
                        update_items.push((
                            item_slot,
                            game_client.inventory.get_item(item_slot).cloned(),
                        ));
                        update_items.extend(material_updates.iter().cloned());
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::CraftUpgradeFailed { update_items })
                            .ok();
                    }
                }
                ClientMessage::CraftNpcUpgradeItem {
                    npc_entity_id,
                    item_slot,
                    ingredients,
                } => {
                    if !client_entity_list
                        .get_zone(game_client.position.zone_id)
                        .and_then(|zone| zone.get_entity(npc_entity_id))
                        .map(|(_, _, npc_position)| npc_position.xy())
                        .map_or(false, |npc_position| {
                            game_client.position.position.xy().distance(npc_position) <= 6000.0
                        })
                    {
                        continue;
                    }

                    let target_item = game_client.inventory.get_item(item_slot).cloned();
                    let target_equip = match &target_item {
                        Some(Item::Equipment(equip)) => Some(equip.clone()),
                        _ => None,
                    };

                    if target_equip.is_none() {
                        continue;
                    }
                    let target_equip = target_equip.unwrap();

                    if target_equip.grade >= 9 {
                        continue;
                    }

                    let target_item_data = match game_data.items.get_base_item(target_equip.item) {
                        Some(target_item_data) => target_item_data,
                        None => continue,
                    };
                    let required_money =
                        upgrade_from_npc_price(target_item_data.quality, target_equip.grade);

                    let required_quantities = match validate_upgrade_materials(
                        &game_data,
                        &game_client.inventory,
                        target_equip.item,
                        target_equip.grade,
                        &ingredients,
                    ) {
                        Ok(required_quantities) => required_quantities,
                        Err(reason) => {
                            warn!(
                                "CraftNpcUpgradeItem invalid materials for target {:?} #{} grade {}: {}",
                                target_equip.item.item_type,
                                target_equip.item.item_number,
                                target_equip.grade,
                                reason
                            );
                            continue;
                        }
                    };

                    if game_client
                        .inventory
                        .try_take_money(required_money)
                        .is_err()
                    {
                        continue;
                    }

                    // Consume exact required ingredient quantities
                    for (ingredient_slot, required_quantity) in
                        ingredients.iter().zip(required_quantities.iter())
                    {
                        if *required_quantity == 0 {
                            continue;
                        }
                        game_client
                            .inventory
                            .try_take_quantity(*ingredient_slot, *required_quantity);
                    }
                    let material_updates = collect_craft_material_inventory_updates(
                        &game_client.inventory,
                        &ingredients,
                    );

                    let success_chance = (90i32 - target_equip.grade as i32 * 8).clamp(10, 95);
                    let roll = rand::random::<i32>().unsigned_abs() as i32 % 100;
                    let success = roll < success_chance;

                    if success {
                        if let Some(slot_ref) = game_client.inventory.get_item_slot_mut(item_slot) {
                            if let Some(Item::Equipment(ref mut equip)) = slot_ref {
                                equip.grade += 1;
                            }
                        }
                        let mut update_items = Vec::new();
                        update_items.push((
                            item_slot,
                            game_client.inventory.get_item(item_slot).cloned(),
                        ));
                        update_items.extend(material_updates.iter().cloned());
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::CraftUpgradeSuccess { update_items })
                            .ok();
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::UpdateMoney {
                                money: game_client.inventory.money,
                            })
                            .ok();
                    } else {
                        if let Some(slot_ref) = game_client.inventory.get_item_slot_mut(item_slot) {
                            if let Some(Item::Equipment(ref mut equip)) = slot_ref {
                                equip.grade = equip.grade.saturating_sub(1);
                            }
                        }
                        let mut update_items = Vec::new();
                        update_items.push((
                            item_slot,
                            game_client.inventory.get_item(item_slot).cloned(),
                        ));
                        update_items.extend(material_updates.iter().cloned());
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::CraftUpgradeFailed { update_items })
                            .ok();
                        game_client
                            .game_client
                            .server_message_tx
                            .send(ServerMessage::UpdateMoney {
                                money: game_client.inventory.money,
                            })
                            .ok();
                    }
                }
                ClientMessage::CraftSkillDisassemble {
                    skill_slot,
                    item_slot,
                } => {
                    // Validate skill
                    let skill_id = game_client.skill_list.get_skill(skill_slot);
                    let skill_data = skill_id.and_then(|id| game_data.skills.get_skill(id));

                    let Some(skill_data) = skill_data else {
                        continue;
                    };

                    if skill_data.skill_type != SkillType::CreateWindow
                        || skill_data.item_make_number != 41
                    {
                        warn!(
                            "CraftSkillDisassemble invalid skill for disassembly: {:?} make_number {}",
                            skill_data.skill_type, skill_data.item_make_number
                        );
                        continue;
                    }

                    let required_mp = manufacture_required_mp(skill_data);
                    if game_client.mana_points.mp < required_mp {
                        warn!(
                            "CraftSkillDisassemble insufficient MP: have {}, need {}",
                            game_client.mana_points.mp, required_mp
                        );
                        continue;
                    }

                    // Get the target item
                    let target_item = game_client.inventory.get_item(item_slot).cloned();
                    let target_item_ref = target_item.as_ref().map(|i| match i {
                        Item::Equipment(e) => e.item,
                        Item::Stackable(s) => s.item,
                    });

                    if target_item_ref.is_none() {
                        continue;
                    }
                    let target_item_ref = target_item_ref.unwrap();

                    // Look up the item's product recipe
                    let target_item_data = game_data.items.get_base_item(target_item_ref);
                    if target_item_data.is_none() {
                        continue;
                    }
                    let target_item_data = target_item_data.unwrap();

                    let product = game_data
                        .products
                        .get_product(target_item_data.craft_material);
                    if product.is_none() {
                        continue;
                    }
                    let product = product.unwrap();

                    // Remove the item being disassembled
                    if let Some(slot_ref) = game_client.inventory.get_item_slot_mut(item_slot) {
                        *slot_ref = None;
                    }

                    let mut update_items: Vec<(ItemSlot, Option<Item>)> = Vec::new();
                    update_items.push((item_slot, None)); // item was removed

                    // Return 50-75% of required materials
                    for mat in &product.materials {
                        let return_pct = 50 + (rand::random::<u32>() % 26); // 50-75%
                        let return_qty =
                            ((mat.quantity as u64 * return_pct as u64) / 100).max(1) as u32;

                        let returned_item = Item::Stackable(StackableItem {
                            item: mat.item,
                            quantity: return_qty,
                        });

                        if let Ok((slot, _)) = game_client.inventory.try_add_item(returned_item) {
                            update_items
                                .push((slot, game_client.inventory.get_item(slot).cloned()));
                        }
                    }

                    game_client.mana_points.mp = (game_client.mana_points.mp - required_mp).max(0);
                    let updated_mp = game_client.mana_points.mp;
                    game_client
                        .game_client
                        .server_message_tx
                        .send(ServerMessage::UpdateAbilityValueSet {
                            ability_type: AbilityType::Mana,
                            value: updated_mp,
                        })
                        .ok();

                    game_client
                        .game_client
                        .server_message_tx
                        .send(ServerMessage::CraftDisassembleSuccess { update_items })
                        .ok();
                }
                ClientMessage::CraftNpcDisassemble {
                    npc_entity_id,
                    item_slot,
                } => {
                    if !client_entity_list
                        .get_zone(game_client.position.zone_id)
                        .and_then(|zone| zone.get_entity(npc_entity_id))
                        .map(|(_, _, npc_position)| npc_position.xy())
                        .map_or(false, |npc_position| {
                            game_client.position.position.xy().distance(npc_position) <= 6000.0
                        })
                    {
                        continue;
                    }

                    // Get the target item
                    let target_item = game_client.inventory.get_item(item_slot).cloned();
                    let target_item_ref = target_item.as_ref().map(|i| match i {
                        Item::Equipment(e) => e.item,
                        Item::Stackable(s) => s.item,
                    });

                    if target_item_ref.is_none() {
                        continue;
                    }
                    let target_item_ref = target_item_ref.unwrap();

                    // Look up the item's product recipe
                    let target_item_data = game_data.items.get_base_item(target_item_ref);
                    if target_item_data.is_none() {
                        continue;
                    }
                    let target_item_data = target_item_data.unwrap();

                    let cost = disassemble_from_npc_price(target_item_data.quality);
                    if game_client.inventory.try_take_money(cost).is_err() {
                        continue;
                    }

                    let product = game_data
                        .products
                        .get_product(target_item_data.craft_material);
                    if product.is_none() {
                        continue;
                    }
                    let product = product.unwrap();

                    // Remove the item being disassembled
                    if let Some(slot_ref) = game_client.inventory.get_item_slot_mut(item_slot) {
                        *slot_ref = None;
                    }

                    let mut update_items: Vec<(ItemSlot, Option<Item>)> = Vec::new();
                    update_items.push((item_slot, None)); // item was removed

                    // Return 50-75% of required materials
                    for mat in &product.materials {
                        let return_pct = 50 + (rand::random::<u32>() % 26); // 50-75%
                        let return_qty =
                            ((mat.quantity as u64 * return_pct as u64) / 100).max(1) as u32;

                        let returned_item = Item::Stackable(StackableItem {
                            item: mat.item,
                            quantity: return_qty,
                        });

                        if let Ok((slot, _)) = game_client.inventory.try_add_item(returned_item) {
                            update_items
                                .push((slot, game_client.inventory.get_item(slot).cloned()));
                        }
                    }

                    game_client
                        .game_client
                        .server_message_tx
                        .send(ServerMessage::CraftDisassembleSuccess { update_items })
                        .ok();
                    game_client
                        .game_client
                        .server_message_tx
                        .send(ServerMessage::UpdateMoney {
                            money: game_client.inventory.money,
                        })
                        .ok();
                }
                ClientMessage::BankOpen => {
                    events.bank_events.send(BankEvent::Open {
                        entity: game_client.entity,
                    });
                }
                ClientMessage::BankDepositItem {
                    item_slot,
                    item,
                    is_premium,
                } => {
                    events.bank_events.send(BankEvent::DepositItem {
                        entity: game_client.entity,
                        item_slot,
                        item,
                        is_premium,
                    });
                }
                ClientMessage::BankWithdrawItem {
                    bank_slot,
                    item,
                    is_premium,
                } => {
                    events.bank_events.send(BankEvent::WithdrawItem {
                        entity: game_client.entity,
                        bank_slot,
                        item,
                        is_premium,
                    });
                }
                ClientMessage::RepairItemUsingNpc {
                    npc_entity_id,
                    item_slot,
                } => {
                    if client_entity_list
                        .get_zone(game_client.position.zone_id)
                        .and_then(|zone| zone.get_entity(npc_entity_id))
                        .map(|(_, _, npc_position)| npc_position.xy())
                        .map_or(false, |npc_position| {
                            game_client.position.position.xy().distance(npc_position) <= 6000.0
                        })
                    {
                        if let Some(Item::Equipment(equipment_item)) =
                            game_client.inventory.get_item(item_slot)
                        {
                            let cost = game_data
                                .ability_value_calculator
                                .calculate_repair_from_npc_price(equipment_item);
                            if game_client.inventory.try_take_money(cost).is_ok() {
                                if let Some(Item::Equipment(equipment_item)) =
                                    game_client.inventory.get_item_mut(item_slot)
                                {
                                    equipment_item.life = 1000;
                                }

                                game_client
                                    .game_client
                                    .server_message_tx
                                    .send(ServerMessage::RepairedItemUsingNpc {
                                        item_slot,
                                        item: game_client
                                            .inventory
                                            .get_item(item_slot)
                                            .unwrap()
                                            .clone(),
                                        updated_money: game_client.inventory.money,
                                    })
                                    .ok();
                            }
                        }
                    }
                }
                ClientMessage::ClanCreate {
                    name,
                    description,
                    mark,
                } => {
                    events.clan_events.send(ClanEvent::Create {
                        creator: game_client.entity,
                        name,
                        description,
                        mark,
                        skip_requirements: false,
                    });
                }
                ClientMessage::ClanInvite { name } => {
                    events.clan_events.send(ClanEvent::Invite {
                        inviter_entity: game_client.entity,
                        name,
                    });
                }
                ClientMessage::ClanSetDescription { description } => {
                    events.clan_events.send(ClanEvent::SetDescription {
                        updater_entity: game_client.entity,
                        description,
                    });
                }
                ClientMessage::ClanAcceptInvite { inviter_name } => {
                    events.clan_events.send(ClanEvent::AcceptInvite {
                        invited_entity: game_client.entity,
                        inviter_name,
                    });
                }
                ClientMessage::ClanRejectInvite { inviter_name } => {
                    events.clan_events.send(ClanEvent::RejectInvite {
                        invited_entity: game_client.entity,
                        inviter_name,
                    });
                }
                ClientMessage::ClanKick { name } => {
                    events.clan_events.send(ClanEvent::Kick {
                        kicker_entity: game_client.entity,
                        name,
                    });
                }
                ClientMessage::ClanPromote { name } => {
                    events.clan_events.send(ClanEvent::Promote {
                        changer_entity: game_client.entity,
                        name,
                    });
                }
                ClientMessage::ClanDemote { name } => {
                    events.clan_events.send(ClanEvent::Demote {
                        changer_entity: game_client.entity,
                        name,
                    });
                }
                ClientMessage::ClanUpgrade { npc_entity_id } => {
                    events.clan_events.send(ClanEvent::Upgrade {
                        requester_entity: game_client.entity,
                        npc_entity_id,
                    });
                }
                ClientMessage::ClanLeave => {
                    events.clan_events.send(ClanEvent::Leave {
                        leaver_entity: game_client.entity,
                    });
                }
                ClientMessage::ClanDisband => {
                    events.clan_events.send(ClanEvent::Disband {
                        entity: game_client.entity,
                    });
                }
                _ => warn!("[GS] Received unimplemented client message {:?}", message),
            }
        }
    }
}
