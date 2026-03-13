use bevy::ecs::prelude::Component;

#[derive(Component, Clone, Copy, Debug)]
pub struct SummonPointCost {
    pub points: u32,
}

impl SummonPointCost {
    pub fn new(points: u32) -> Self {
        Self { points }
    }
}
