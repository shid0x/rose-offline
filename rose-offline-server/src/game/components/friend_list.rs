use bevy::ecs::prelude::Component;
use rose_game_common::messages::Friend;

#[derive(Clone, Component, Default)]
pub struct FriendList {
    pub friends: Vec<Friend>,
}
