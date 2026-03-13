use bevy::{ecs::prelude::Component, reflect::Reflect};
use serde::{Deserialize, Serialize};

#[derive(
    Component, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflect,
)]
pub struct RecoveryRateBonus {
    pub hp_bonus: i32,
    pub mp_bonus: i32,
}

impl RecoveryRateBonus {
    pub fn new(hp_bonus: i32, mp_bonus: i32) -> Self {
        Self { hp_bonus, mp_bonus }
    }
}
