use bevy::ecs::prelude::Component;

#[derive(Component, Clone, Debug, Default)]
pub struct SummonUsage {
    pub used_points: u32,
}
