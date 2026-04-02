use bevy::ecs::prelude::Component;

#[derive(Component, Clone, Copy, Debug)]
pub struct SummonFollowSpeedOverride {
    pub speed: f32,
}

impl SummonFollowSpeedOverride {
    pub fn new(speed: f32) -> Self {
        Self { speed }
    }
}
