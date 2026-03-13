use std::time::Duration;

use bevy::{ecs::prelude::Component, prelude::Entity};

pub const BONFIRE_OWNER_LEASH_DISTANCE: f32 = 2500.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BonfireAuraTier {
    pub min_nearby_allies: usize,
    pub search_radius: f32,
    pub effect_radius: f32,
    pub hp_bonus: i32,
    pub mp_bonus: i32,
}

impl BonfireAuraTier {
    pub fn new(
        min_nearby_allies: usize,
        search_radius: f32,
        effect_radius: f32,
        hp_bonus: i32,
        mp_bonus: i32,
    ) -> Self {
        Self {
            min_nearby_allies,
            search_radius,
            effect_radius,
            hp_bonus,
            mp_bonus,
        }
    }
}

#[derive(Component, Clone, Debug)]
pub struct BonfireAura {
    pub owner_entity: Entity,
    pub owner_party: Option<Entity>,
    pub pulse_interval: Duration,
    pub elapsed: Duration,
    pub tiers: Vec<BonfireAuraTier>,
}

impl BonfireAura {
    pub fn new(
        owner_entity: Entity,
        owner_party: Option<Entity>,
        pulse_interval: Duration,
        tiers: Vec<BonfireAuraTier>,
    ) -> Self {
        Self {
            owner_entity,
            owner_party,
            pulse_interval,
            elapsed: Duration::ZERO,
            tiers,
        }
    }

    pub fn pulse_immediately(&mut self) {
        self.elapsed = self.pulse_interval;
    }
}
