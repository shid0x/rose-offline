use rose_data::ZoneData;

pub const ZONE_FLAG_PK_ALLOWED: u32 = 0x1;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ZonePvpMode {
    Disabled,
    FreeForAll,
    TeamBased,
}

pub fn zone_pvp_mode_from_state(pvp_state: u32) -> ZonePvpMode {
    match pvp_state {
        1 => ZonePvpMode::FreeForAll,
        2 | 11 => ZonePvpMode::TeamBased,
        _ => ZonePvpMode::Disabled,
    }
}

pub fn zone_is_initially_pvp_enabled(zone_data: &ZoneData) -> bool {
    !matches!(
        zone_pvp_mode_from_state(zone_data.pvp_state),
        ZonePvpMode::Disabled
    )
}

pub fn join_zone_global_flags(zone_data: &ZoneData, pvp_enabled: bool) -> u32 {
    let pvp_allowed = !matches!(
        zone_pvp_mode_from_state(zone_data.pvp_state),
        ZonePvpMode::Disabled
    ) && pvp_enabled;
    if pvp_allowed {
        ZONE_FLAG_PK_ALLOWED
    } else {
        0
    }
}

pub fn can_character_attack_character(
    zone_data: &ZoneData,
    pvp_enabled: bool,
    attacker_team_id: u32,
    defender_team_id: u32,
) -> bool {
    if !pvp_enabled {
        return false;
    }

    match zone_pvp_mode_from_state(zone_data.pvp_state) {
        ZonePvpMode::Disabled => false,
        ZonePvpMode::FreeForAll => true,
        ZonePvpMode::TeamBased => attacker_team_id != defender_team_id,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        can_character_attack_character, zone_pvp_mode_from_state, ZonePvpMode, ZONE_FLAG_PK_ALLOWED,
    };

    #[test]
    fn zone_pvp_mode_mapping_matches_spec() {
        assert_eq!(zone_pvp_mode_from_state(0), ZonePvpMode::Disabled);
        assert_eq!(zone_pvp_mode_from_state(1), ZonePvpMode::FreeForAll);
        assert_eq!(zone_pvp_mode_from_state(2), ZonePvpMode::TeamBased);
        assert_eq!(zone_pvp_mode_from_state(11), ZonePvpMode::TeamBased);
        assert_eq!(zone_pvp_mode_from_state(999), ZonePvpMode::Disabled);
    }

    #[test]
    fn pvp_allow_matrix_matches_rules() {
        struct Case {
            zone_state: u32,
            pvp_enabled: bool,
            attacker_team: u32,
            defender_team: u32,
            expected: bool,
        }

        let cases = [
            Case {
                zone_state: 0,
                pvp_enabled: true,
                attacker_team: 2,
                defender_team: 3,
                expected: false,
            },
            Case {
                zone_state: 1,
                pvp_enabled: false,
                attacker_team: 2,
                defender_team: 2,
                expected: false,
            },
            Case {
                zone_state: 1,
                pvp_enabled: true,
                attacker_team: 2,
                defender_team: 2,
                expected: true,
            },
            Case {
                zone_state: 2,
                pvp_enabled: true,
                attacker_team: 2,
                defender_team: 2,
                expected: false,
            },
            Case {
                zone_state: 2,
                pvp_enabled: true,
                attacker_team: 2,
                defender_team: 3,
                expected: true,
            },
            Case {
                zone_state: 11,
                pvp_enabled: true,
                attacker_team: 7,
                defender_team: 8,
                expected: true,
            },
        ];

        for case in cases {
            let zone_data = rose_data::ZoneData {
                id: rose_data::ZoneId::new(1).unwrap(),
                name: "",
                description: "",
                pvp_state: case.zone_state,
                join_trigger: None,
                kill_trigger: None,
                dead_trigger: None,
                sector_size: 5000,
                grid_per_patch: 1.0,
                grid_size: 1.0,
                event_objects: Vec::new(),
                monster_spawns: Vec::new(),
                npcs: Vec::new(),
                sectors_base_position: bevy::math::Vec2::ZERO,
                num_sectors_x: 1,
                num_sectors_y: 1,
                start_position: bevy::math::Vec3::ZERO,
                revive_positions: Vec::new(),
                event_positions: std::collections::HashMap::new(),
                day_cycle: 0,
                morning_time: 0,
                day_time: 0,
                evening_time: 0,
                night_time: 0,
                skybox_id: None,
                party_xp_a: 2,
                party_xp_b: 2,
            };

            assert_eq!(
                can_character_attack_character(
                    &zone_data,
                    case.pvp_enabled,
                    case.attacker_team,
                    case.defender_team
                ),
                case.expected
            );
        }
    }

    #[test]
    fn global_flag_constant_is_stable() {
        assert_eq!(ZONE_FLAG_PK_ALLOWED, 0x1);
    }
}
