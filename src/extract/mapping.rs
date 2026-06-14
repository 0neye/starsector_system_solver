//! campaign.xml extraction rows mapped into solver/DB-friendly records.

use std::collections::HashMap;

use crate::extract::gamedata::GameData;
use crate::extract::model::{
    InfraRow, MappedOutput, MappedSystem, PlanetRow, RawPlanet, RawSave, RawSystem, SystemRow,
    UnknownCondition,
};

const LY_TO_RAW_UNITS: f64 = 2000.0;

pub fn map_save(raw: RawSave, game_data: &GameData) -> MappedOutput {
    // Colonized markets mix colony-management conditions (population_*,
    // pirate_activity, ...) into the planetary list; keep only conditions known
    // to be planetary so they don't pollute hazard sums or unknown tracking.
    let mut raw = raw;
    for system in &mut raw.systems {
        for planet in &mut system.planets {
            if planet.owner_faction.is_some() || planet.market_size > 0 {
                planet
                    .conditions
                    .retain(|condition| game_data.is_planetary_condition(condition));
            }
        }
    }
    let raw = raw;

    let mut unknown_conditions: HashMap<String, UnknownCondition> = HashMap::new();
    let mut com_weighted_sum = (0.0_f64, 0.0_f64);
    let mut com_weight = 0.0_f64;

    for system in &raw.systems {
        for planet in &system.planets {
            if is_star_like(&planet.planet_type, game_data) {
                continue;
            }
            if let (Some((x, y)), Some(owner)) = (system.hyper_loc, planet.owner_faction.as_deref())
            {
                if planet.market_size > 0 && owner != "player" {
                    let weight = planet.market_size as f64;
                    com_weighted_sum.0 += x * weight;
                    com_weighted_sum.1 += y * weight;
                    com_weight += weight;
                }
            }
            for condition in &planet.conditions {
                // Planetary-flagged conditions without procgen data (world types
                // like `jungle`/`arid`) are known, they just contribute 0 hazard.
                if !game_data.is_planetary_condition(condition) {
                    let entry = unknown_conditions
                        .entry(condition.clone())
                        .or_insert_with(|| UnknownCondition {
                            condition_id: condition.clone(),
                            occurrences: 0,
                            example_planet: planet.name.clone(),
                        });
                    entry.occurrences += 1;
                }
            }
        }
    }

    let com = if com_weight > 0.0 {
        Some((
            com_weighted_sum.0 / com_weight,
            com_weighted_sum.1 / com_weight,
        ))
    } else {
        None
    };

    let systems = raw
        .systems
        .into_iter()
        .map(|system| map_system(system, game_data, com))
        .collect();

    let mut unknown_conditions: Vec<UnknownCondition> = unknown_conditions.into_values().collect();
    unknown_conditions.sort_by(|a, b| a.condition_id.cmp(&b.condition_id));

    MappedOutput {
        systems,
        player: raw.player,
        unknown_conditions,
    }
}

fn map_system(system: RawSystem, game_data: &GameData, com: Option<(f64, f64)>) -> MappedSystem {
    let dist_from_com_ly = system
        .hyper_loc
        .zip(com)
        .map(|(loc, com)| distance_ly(loc, com));
    let x_ly = system.hyper_loc.map(|loc| loc.0 / LY_TO_RAW_UNITS);
    let y_ly = system.hyper_loc.map(|loc| loc.1 / LY_TO_RAW_UNITS);
    let stable_points = count_stable_points(&system);
    let remnant_state = remnant_state(&system);
    let has_gate = system
        .entities
        .iter()
        .any(|entity| entity.spec_id == "inactive_gate");

    let mut infrastructure = Vec::new();
    for entity in &system.entities {
        match entity.spec_id.as_str() {
            "comm_relay" => infrastructure.push(InfraRow {
                infrastructure_type: "CommRelay".to_string(),
                is_domain: true,
                is_damaged: false,
            }),
            "comm_relay_makeshift" => infrastructure.push(InfraRow {
                infrastructure_type: "CommRelay".to_string(),
                is_domain: false,
                is_damaged: false,
            }),
            "nav_buoy" => infrastructure.push(InfraRow {
                infrastructure_type: "NavBouy".to_string(),
                is_domain: true,
                is_damaged: false,
            }),
            "nav_buoy_makeshift" => infrastructure.push(InfraRow {
                infrastructure_type: "NavBouy".to_string(),
                is_domain: false,
                is_damaged: false,
            }),
            "sensor_array" => infrastructure.push(InfraRow {
                infrastructure_type: "SensorArray".to_string(),
                is_domain: true,
                is_damaged: false,
            }),
            "sensor_array_makeshift" => infrastructure.push(InfraRow {
                infrastructure_type: "SensorArray".to_string(),
                is_domain: false,
                is_damaged: false,
            }),
            "inactive_gate" => infrastructure.push(InfraRow {
                infrastructure_type: "Gate".to_string(),
                is_domain: false,
                is_damaged: false,
            }),
            "coronal_tap" => infrastructure.push(InfraRow {
                infrastructure_type: "CoronalTap".to_string(),
                is_domain: false,
                is_damaged: false,
            }),
            "derelict_cryosleeper" => infrastructure.push(InfraRow {
                infrastructure_type: "Cryosleeper".to_string(),
                is_domain: false,
                is_damaged: true,
            }),
            _ => {}
        }
    }
    if remnant_state.0 {
        infrastructure.push(InfraRow {
            infrastructure_type: "Remnants".to_string(),
            is_domain: false,
            is_damaged: remnant_state.1,
        });
    }

    let tags = system.tags.clone();
    let planets = system
        .planets
        .into_iter()
        .map(|planet| map_planet(&planet, game_data, dist_from_com_ly))
        .collect();

    MappedSystem {
        system: SystemRow {
            name: system.name,
            display_name: system.display_name,
            internal_id: system.internal_id,
            x_ly,
            y_ly,
            dist_from_com_ly,
            stable_points,
            has_gate,
            has_remnants: remnant_state.0,
            remnant_damaged: remnant_state.1,
            star_types: system.star_types,
            tags,
        },
        planets,
        infrastructure,
    }
}

fn map_planet(
    planet: &RawPlanet,
    game_data: &GameData,
    dist_from_com_ly: Option<f64>,
) -> PlanetRow {
    let accessibility_percent =
        dist_from_com_ly.map(|dist| (100.0 * (1.0 - dist / 50.0)).round().max(0.0));

    let ruins = resource_value(
        &planet.conditions,
        "ruins_scattered",
        "ruins_widespread",
        "ruins_extensive",
        "ruins_vast",
    );
    let farmland = resource_value(
        &planet.conditions,
        "farmland_poor",
        "farmland_adequate",
        "farmland_rich",
        "farmland_bountiful",
    );
    let rare_ores = resource_value(
        &planet.conditions,
        "rare_ore_sparse",
        "rare_ore_moderate",
        "rare_ore_abundant",
        "rare_ore_rich",
    )
    .or_else(|| resource_value_ultrarich(&planet.conditions, "rare_ore_ultrarich"));
    let ores = resource_value(
        &planet.conditions,
        "ore_sparse",
        "ore_moderate",
        "ore_abundant",
        "ore_rich",
    )
    .or_else(|| resource_value_ultrarich(&planet.conditions, "ore_ultrarich"));
    let volatiles = resource_value(
        &planet.conditions,
        "volatiles_trace",
        "volatiles_diffuse",
        "volatiles_abundant",
        "volatiles_plentiful",
    );
    let organics = resource_value(
        &planet.conditions,
        "organics_trace",
        "organics_common",
        "organics_abundant",
        "organics_plentiful",
    );
    let water = has_condition(planet, "water_surface");

    PlanetRow {
        name: planet.name.clone(),
        internal_id: planet.internal_id.clone(),
        planet_type: planet.planet_type.clone(),
        is_moon: planet.is_moon,
        survey_level: planet.survey_level.clone(),
        owner_faction: planet.owner_faction.clone(),
        radius: planet.radius,
        ruins,
        farmland,
        rare_ores,
        ores,
        volatiles,
        organics,
        accessibility_percent,
        hazard_percent: hazard_percent(planet, game_data),
        hazard_incomplete: planet
            .conditions
            .iter()
            .any(|condition| !game_data.is_planetary_condition(condition)),
        no_atmosphere: has_condition(planet, "no_atmosphere"),
        very_hot: has_condition(planet, "very_hot"),
        gas_giant: planet.tags.iter().any(|tag| tag == "gas_giant")
            || planet.planet_type == "gas_giant",
        habitable: has_condition(planet, "habitable"),
        extreme_activity: has_condition(planet, "extreme_tectonic_activity"),
        water,
        conditions: planet.conditions.clone(),
    }
}

fn hazard_percent(planet: &RawPlanet, game_data: &GameData) -> f64 {
    let hazard_sum: f64 = planet
        .conditions
        .iter()
        .map(|condition| game_data.condition_hazard(condition).unwrap_or(0.0))
        .sum();
    100.0 + 100.0 * hazard_sum
}

fn resource_value(
    conditions: &[String],
    negative_one: &str,
    zero: &str,
    one: &str,
    two: &str,
) -> Option<f64> {
    if conditions.iter().any(|condition| condition == two) {
        Some(2.0)
    } else if conditions.iter().any(|condition| condition == one) {
        Some(1.0)
    } else if conditions.iter().any(|condition| condition == zero) {
        Some(0.0)
    } else if conditions.iter().any(|condition| condition == negative_one) {
        Some(-1.0)
    } else {
        None
    }
}

fn resource_value_ultrarich(conditions: &[String], condition: &str) -> Option<f64> {
    if conditions.iter().any(|entry| entry == condition) {
        Some(3.0)
    } else {
        None
    }
}

fn has_condition(planet: &RawPlanet, condition: &str) -> bool {
    planet.conditions.iter().any(|entry| entry == condition)
}

fn count_stable_points(system: &RawSystem) -> u32 {
    system
        .entities
        .iter()
        .filter(|entity| {
            matches!(
                entity.spec_id.as_str(),
                "stable_location"
                    | "comm_relay"
                    | "comm_relay_makeshift"
                    | "nav_buoy"
                    | "nav_buoy_makeshift"
                    | "sensor_array"
                    | "sensor_array_makeshift"
            )
        })
        .count() as u32
}

fn remnant_state(system: &RawSystem) -> (bool, bool) {
    if system
        .tags
        .iter()
        .any(|tag| tag == "theme_remnant_resurgent")
    {
        return (true, false);
    }
    if system
        .tags
        .iter()
        .any(|tag| tag == "theme_remnant_suppressed")
    {
        return (true, true);
    }
    (false, false)
}

fn distance_ly(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dx = (a.0 - b.0) / LY_TO_RAW_UNITS;
    let dy = (a.1 - b.1) / LY_TO_RAW_UNITS;
    (dx * dx + dy * dy).sqrt()
}

fn is_star_like(type_id: &str, game_data: &GameData) -> bool {
    game_data.is_star_type(type_id) || is_star_like_raw(type_id)
}

fn is_star_like_raw(type_id: &str) -> bool {
    type_id.starts_with("star_") || type_id == "black_hole" || type_id.starts_with("nebula_center_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::gamedata::{ConditionSpec, GameData, PlanetTypeSpec};
    use crate::extract::model::{RawEntity, RawPlanet, RawSave, RawSystem};

    fn sample_game_data() -> GameData {
        let mut conditions = HashMap::new();
        for (id, feature_key, hazard) in [
            ("habitable", "habitable:1", -0.25),
            ("water_surface", "surface:1", 0.25),
            ("ore_moderate", "ore:2", 0.0),
            ("rare_ore_rich", "rare_ore:4", 0.5),
            ("very_hot", "hot:2", 0.5),
        ] {
            conditions.insert(
                id.to_string(),
                ConditionSpec {
                    id: id.to_string(),
                    group: String::new(),
                    rank: None,
                    hazard,
                    source: "vanilla".to_string(),
                    feature_key: feature_key.to_string(),
                },
            );
        }

        let mut planet_types = HashMap::new();
        planet_types.insert(
            "water".to_string(),
            PlanetTypeSpec {
                source: "vanilla".to_string(),
                is_star_like: false,
            },
        );
        planet_types.insert(
            "desert".to_string(),
            PlanetTypeSpec {
                source: "vanilla".to_string(),
                is_star_like: false,
            },
        );
        planet_types.insert(
            "mod_water".to_string(),
            PlanetTypeSpec {
                source: "Mod".to_string(),
                is_star_like: false,
            },
        );

        GameData {
            conditions,
            planet_types,
            planetary_conditions: Default::default(),
        }
    }

    fn planet(name: &str, planet_type: &str, conditions: &[&str], colonized: bool) -> RawPlanet {
        RawPlanet {
            name: name.to_string(),
            internal_id: Some(name.to_string()),
            planet_type: planet_type.to_string(),
            radius: 100.0,
            tags: if planet_type == "water" || planet_type == "mod_water" {
                vec!["gas_giant".to_string()]
            } else {
                vec![]
            },
            conditions: conditions.iter().map(|s| s.to_string()).collect(),
            survey_level: Some("FULL".to_string()),
            owner_faction: if colonized {
                Some("hegemony".to_string())
            } else {
                None
            },
            market_size: if colonized { 3 } else { 0 },
            is_moon: false,
        }
    }

    #[test]
    fn maps_hazard_accessibility_and_save_derived_flags() {
        let raw = RawSave {
            player: None,
            systems: vec![
                RawSystem {
                    name: "Alpha".to_string(),
                    display_name: "Alpha Star System".to_string(),
                    internal_id: "1".to_string(),
                    hyper_loc: Some((0.0, 0.0)),
                    tags: vec![],
                    star_types: vec!["star_yellow".to_string()],
                    planets: vec![planet("A", "water", &["habitable", "water_surface"], true)],
                    entities: vec![RawEntity {
                        spec_id: "stable_location".to_string(),
                        name: None,
                    }],
                },
                RawSystem {
                    name: "Beta".to_string(),
                    display_name: "Beta Star System".to_string(),
                    internal_id: "2".to_string(),
                    hyper_loc: Some((2000.0, 0.0)),
                    tags: vec![
                        "theme_outer".to_string(),
                        "theme_remnant_suppressed".to_string(),
                    ],
                    star_types: vec!["star_yellow".to_string()],
                    planets: vec![planet(
                        "B",
                        "mod_water",
                        &[
                            "habitable",
                            "water_surface",
                            "ore_moderate",
                            "rare_ore_rich",
                            "very_hot",
                        ],
                        true,
                    )],
                    entities: vec![RawEntity {
                        spec_id: "comm_relay_makeshift".to_string(),
                        name: None,
                    }],
                },
            ],
        };

        let mapped = map_save(raw, &sample_game_data());
        assert_eq!(mapped.systems.len(), 2);
        assert_eq!(mapped.unknown_conditions.len(), 0);
        let alpha = &mapped.systems[0];
        assert_eq!(alpha.system.stable_points, 1);
        // habitable (-0.25) + water_surface (+0.25) cancel out
        assert_eq!(alpha.planets[0].hazard_percent, 100.0);
        // COM is midway between the two size-3 colonies: 1000 units = 0.5 LY away
        assert_eq!(alpha.planets[0].accessibility_percent, Some(99.0));
        assert!(alpha.planets[0].gas_giant);
        let beta = &mapped.systems[1];
        assert_eq!(
            beta.system.tags,
            vec![
                "theme_outer".to_string(),
                "theme_remnant_suppressed".to_string()
            ]
        );
        assert!(beta.system.has_remnants);
        assert!(beta.system.remnant_damaged);
        assert_eq!(beta.planets[0].ores, Some(0.0));
        assert_eq!(beta.planets[0].rare_ores, Some(2.0));
        assert!(beta.planets[0].water);
    }
}
