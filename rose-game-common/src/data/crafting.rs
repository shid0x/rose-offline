use rose_data::{AbilityType, SkillData};

use crate::components::Money;

pub fn manufacture_required_mp(skill_data: &SkillData) -> i32 {
    skill_data
        .use_ability
        .iter()
        .find_map(|&(ability_type, ability_value)| {
            (ability_type == AbilityType::Mana).then_some(ability_value)
        })
        .map_or(0, |mp| mp.max(0))
}

pub fn manufacture_success_chance(
    skill_level: u32,
    required_skill_level: u32,
    world_craft_rate: i32,
) -> i32 {
    let level_gap = skill_level.saturating_sub(required_skill_level) as i32;
    let base = 92 + level_gap * 2;
    let scaled = base * world_craft_rate / 100;
    scaled.clamp(60, 99)
}

pub fn disassemble_from_npc_price(item_quality: u32) -> Money {
    Money(item_quality as i64 * 10 + 20)
}

pub fn upgrade_from_npc_price(item_quality: u32, item_grade: u8) -> Money {
    let item_quality = item_quality as i64;
    let item_grade = item_grade as i64;
    Money(item_grade * (item_grade + 1) * item_quality * (item_quality + 20) / 5)
}

#[cfg(test)]
mod tests {
    use super::{disassemble_from_npc_price, upgrade_from_npc_price};
    use crate::components::Money;

    #[test]
    fn disassemble_npc_price_matches_original_formula() {
        assert_eq!(disassemble_from_npc_price(0), Money(20));
        assert_eq!(disassemble_from_npc_price(42), Money(440));
    }

    #[test]
    fn upgrade_npc_price_matches_original_formula() {
        assert_eq!(upgrade_from_npc_price(50, 0), Money(0));
        assert_eq!(upgrade_from_npc_price(50, 3), Money(8_400));
    }
}
