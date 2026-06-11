//! campaign.xml extraction rows mapped into solver/DB-friendly records.

use std::collections::{HashMap, HashSet};

use crate::extract::gamedata::GameData;
use crate::extract::model::{
    InfraRow, MappedOutput, MappedSystem, PlanetRow, RawPlanet, RawSave, RawSystem, SystemRow,
    TypeMapping, UnknownCondition,
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

    let type_profiles = build_type_profiles(&raw, game_data);
    let vanilla_types: HashSet<_> = game_data
        .planet_types
        .iter()
        .filter(|(_, spec)| spec.source == "vanilla" && !spec.is_star_like)
        .map(|(id, _)| id.clone())
        .collect();

    let mut mapped_type_for_raw: HashMap<String, Option<String>> = HashMap::new();
    let mut type_mappings = Vec::new();
    let mut unknown_types = HashSet::new();

    for (type_id, profile) in &type_profiles {
        let known = game_data.planet_types.get(type_id);
        if known.is_none() {
            unknown_types.insert(type_id.clone());
            mapped_type_for_raw.insert(type_id.clone(), None);
            continue;
        }

        let spec = known.unwrap();
        if spec.source == "vanilla" {
            mapped_type_for_raw.insert(type_id.clone(), Some(type_id.clone()));
            continue;
        }

        // No planets of this type in the save (or none with surveyed conditions)
        // means there is no statistical evidence to map from.
        if profile.sample_count == 0 || profile.features.is_empty() {
            mapped_type_for_raw.insert(type_id.clone(), None);
            continue;
        }

        let mut candidates: Vec<&TypeProfile> = type_profiles
            .values()
            .filter(|candidate| {
                candidate.type_id != *type_id
                    && vanilla_types.contains(&candidate.type_id)
                    && !candidate.is_star_like
            })
            .collect();

        let gas_filtered: Vec<&TypeProfile> = candidates
            .iter()
            .copied()
            .filter(|candidate| candidate.is_gas_giant == profile.is_gas_giant)
            .collect();
        if !gas_filtered.is_empty() {
            candidates = gas_filtered;
        }

        let hab_filtered: Vec<&TypeProfile> = candidates
            .iter()
            .copied()
            .filter(|candidate| candidate.is_majority_habitable == profile.is_majority_habitable)
            .collect();
        if !hab_filtered.is_empty() {
            candidates = hab_filtered;
        }

        let mut scored: Vec<TypeScore> = candidates
            .into_iter()
            .map(|candidate| TypeScore {
                type_id: candidate.type_id.clone(),
                similarity: cosine_similarity(&profile.features, &candidate.features),
                samples: candidate.sample_count,
            })
            .collect();
        scored.retain(|score| score.similarity > 0.0);
        scored.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.type_id.cmp(&b.type_id))
        });

        mapped_type_for_raw.insert(
            type_id.clone(),
            scored.first().map(|best| best.type_id.clone()),
        );
        for score in scored.into_iter().take(3) {
            type_mappings.push(TypeMapping {
                modded_type: type_id.clone(),
                vanilla_type: score.type_id,
                similarity: score.similarity,
                modded_samples: profile.sample_count,
                vanilla_samples: score.samples,
            });
        }
    }

    type_mappings.sort_by(|a, b| {
        a.modded_type
            .cmp(&b.modded_type)
            .then_with(|| {
                b.similarity
                    .partial_cmp(&a.similarity)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.vanilla_type.cmp(&b.vanilla_type))
    });

    let systems = raw
        .systems
        .into_iter()
        .map(|system| map_system(system, game_data, com, &mapped_type_for_raw))
        .collect();

    let mut unknown_conditions: Vec<UnknownCondition> = unknown_conditions.into_values().collect();
    unknown_conditions.sort_by(|a, b| a.condition_id.cmp(&b.condition_id));

    let mut unknown_types: Vec<String> = unknown_types.into_iter().collect();
    unknown_types.sort();

    MappedOutput {
        systems,
        unknown_conditions,
        type_mappings,
        unknown_types,
    }
}

#[derive(Debug, Clone)]
struct TypeStats {
    features: HashMap<String, f64>,
    sample_count: u32,
    habitable_count: u32,
    gas_count: u32,
}

impl TypeStats {
    fn new() -> Self {
        Self {
            features: HashMap::new(),
            sample_count: 0,
            habitable_count: 0,
            gas_count: 0,
        }
    }
}

#[derive(Debug, Clone)]
struct TypeProfile {
    type_id: String,
    features: HashMap<String, f64>,
    sample_count: u32,
    is_gas_giant: bool,
    is_majority_habitable: bool,
    is_star_like: bool,
}

#[derive(Debug, Clone)]
struct TypeScore {
    type_id: String,
    similarity: f64,
    samples: u32,
}

fn build_type_profiles(raw: &RawSave, game_data: &GameData) -> HashMap<String, TypeProfile> {
    let mut stats: HashMap<String, TypeStats> = HashMap::new();

    for system in &raw.systems {
        for planet in &system.planets {
            if is_star_like(&planet.planet_type, game_data) {
                continue;
            }
            let entry = stats
                .entry(planet.planet_type.clone())
                .or_insert_with(TypeStats::new);
            entry.sample_count += 1;
            if planet.conditions.iter().any(|c| c == "habitable") {
                entry.habitable_count += 1;
            }
            if planet.tags.iter().any(|tag| tag == "gas_giant") {
                entry.gas_count += 1;
            }
            for condition in &planet.conditions {
                let feature = game_data
                    .condition_feature_key(condition)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| condition.clone());
                *entry.features.entry(feature).or_insert(0.0) += 1.0;
            }
        }
    }

    let mut profiles = HashMap::new();
    for (type_id, stat) in stats {
        profiles.insert(
            type_id.clone(),
            TypeProfile {
                type_id,
                features: normalize_features(stat.features, stat.sample_count),
                sample_count: stat.sample_count,
                is_gas_giant: stat.gas_count * 2 >= stat.sample_count.max(1),
                is_majority_habitable: stat.habitable_count * 2 >= stat.sample_count.max(1),
                is_star_like: false,
            },
        );
    }

    for (type_id, spec) in &game_data.planet_types {
        if spec.is_star_like || profiles.contains_key(type_id) {
            continue;
        }
        profiles.insert(
            type_id.clone(),
            TypeProfile {
                type_id: type_id.clone(),
                features: HashMap::new(),
                sample_count: 0,
                is_gas_giant: false,
                is_majority_habitable: false,
                is_star_like: spec.is_star_like,
            },
        );
    }

    for profile in profiles.values_mut() {
        if let Some(spec) = game_data.planet_types.get(&profile.type_id) {
            profile.is_star_like = spec.is_star_like || is_star_like_raw(&profile.type_id);
        } else {
            profile.is_star_like = is_star_like_raw(&profile.type_id);
        }
    }

    profiles
}

fn normalize_features(features: HashMap<String, f64>, sample_count: u32) -> HashMap<String, f64> {
    if sample_count == 0 {
        return features;
    }
    let denom = sample_count as f64;
    features
        .into_iter()
        .map(|(key, value)| (key, value / denom))
        .collect()
}

fn map_system(
    system: RawSystem,
    game_data: &GameData,
    com: Option<(f64, f64)>,
    mapped_type_for_raw: &HashMap<String, Option<String>>,
) -> MappedSystem {
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
        .map(|planet| map_planet(&planet, game_data, mapped_type_for_raw, dist_from_com_ly))
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
    mapped_type_for_raw: &HashMap<String, Option<String>>,
    dist_from_com_ly: Option<f64>,
) -> PlanetRow {
    let mapped_vanilla_type = mapped_type_for_raw
        .get(&planet.planet_type)
        .cloned()
        .unwrap_or_else(|| {
            if game_data.planet_types.contains_key(&planet.planet_type)
                && !game_data.is_star_type(&planet.planet_type)
            {
                Some(planet.planet_type.clone())
            } else {
                None
            }
        });

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
    let water =
        has_condition(planet, "water_surface") || mapped_vanilla_type.as_deref() == Some("water");

    PlanetRow {
        name: planet.name.clone(),
        internal_id: planet.internal_id.clone(),
        planet_type: planet.planet_type.clone(),
        mapped_vanilla_type,
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

fn cosine_similarity(a: &HashMap<String, f64>, b: &HashMap<String, f64>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut a_norm = 0.0;
    let mut b_norm = 0.0;
    for value in a.values() {
        a_norm += value * value;
    }
    for value in b.values() {
        b_norm += value * value;
    }
    for (key, a_val) in a {
        if let Some(b_val) = b.get(key) {
            dot += a_val * b_val;
        }
    }
    if a_norm <= f64::EPSILON || b_norm <= f64::EPSILON {
        0.0
    } else {
        dot / (a_norm.sqrt() * b_norm.sqrt())
    }
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
    fn maps_hazard_accessibility_and_type_similarity() {
        let raw = RawSave {
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
        assert_eq!(
            beta.planets[0].mapped_vanilla_type.as_deref(),
            Some("water")
        );
        assert!(mapped
            .type_mappings
            .iter()
            .any(|m| m.modded_type == "mod_water" && m.vanilla_type == "water"));
    }
}
