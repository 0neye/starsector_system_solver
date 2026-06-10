use std::collections::HashMap;
use std::error::Error;
use std::path::Path;

use csv;
use rustc_hash::FxHashMap;

use crate::extract::db::Db;
use crate::planet::Planet;
use crate::system::{System, Infrastructure};
use crate::constants::AdminType;

#[derive(Debug)]
pub enum ParserError {
    MissingColumn(String),
    InvalidValue(String),
    IoError(std::io::Error),
    CsvError(csv::Error),
    DbError(crate::extract::ExtractError),
}

impl std::fmt::Display for ParserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParserError::MissingColumn(col) => write!(f, "Missing column: {}", col),
            ParserError::InvalidValue(msg) => write!(f, "Invalid value: {}", msg),
            ParserError::IoError(e) => write!(f, "IO error: {}", e),
            ParserError::CsvError(e) => write!(f, "CSV error: {}", e),
            ParserError::DbError(e) => write!(f, "DB error: {}", e),
        }
    }
}

impl Error for ParserError {}

impl From<std::io::Error> for ParserError {
    fn from(err: std::io::Error) -> ParserError {
        ParserError::IoError(err)
    }
}

impl From<csv::Error> for ParserError {
    fn from(err: csv::Error) -> ParserError {
        ParserError::CsvError(err)
    }
}

impl From<crate::extract::ExtractError> for ParserError {
    fn from(err: crate::extract::ExtractError) -> ParserError {
        ParserError::DbError(err)
    }
}

pub fn parse_planets_csv<P: AsRef<Path>>(path: P) -> Result<(HashMap<String, Planet>, HashMap<String, System>), ParserError> {
    let mut planets = HashMap::new();
    let mut systems = HashMap::new();
    let mut rdr = csv::Reader::from_path(path)?;
    let headers = rdr.headers()?.clone();
    
    for result in rdr.records() {
        let record = result?;
        let planet_name = record.get(0)
            .ok_or_else(|| ParserError::MissingColumn("Planet Name".to_string()))?
            .to_string();
        let system_name = record.get(1)
            .ok_or_else(|| ParserError::MissingColumn("System Name".to_string()))?
            .to_string();
        
        // Parse properties from CSV columns
        let mut properties = FxHashMap::default();
        
        // Skip the first two columns (planet name, system name) and parse the rest as properties
        for (i, value) in record.iter().skip(2).enumerate() {
            if let Some(header) = headers.get(i + 2) {
                // Skip empty cells
                if value.trim().is_empty() {
                    continue;
                }
                
                // Try parsing as float first
                if let Ok(float_val) = value.parse::<f64>() {
                    properties.insert(header.to_lowercase(), float_val);
                    continue;
                }
                
                // Try parsing as boolean
                match value.to_uppercase().as_str() {
                    "TRUE" => properties.insert(header.to_lowercase(), 1.0),
                    "FALSE" => properties.insert(header.to_lowercase(), 0.0),
                    _ => None
                };
            }
        }
        
        // Create planet and add it to both collections
        let planet = Planet::new(planet_name.clone(), properties);
        planets.insert(planet_name.clone(), planet.clone());
        
        let system = systems
            .entry(system_name.clone())
            .or_insert_with(|| System::new(system_name.clone()));
        system.add_planet(planet);
    }
    
    Ok((planets, systems))
}

pub fn parse_infrastructure_csv<P: AsRef<Path>>(
    path: P,
    systems: &mut HashMap<String, System>
) -> Result<(), ParserError> {
    let mut rdr = csv::Reader::from_path(path)?;
    
    for result in rdr.records() {
        let record = result?;
        let system_name = record.get(0)
            .ok_or_else(|| ParserError::MissingColumn("system_name".to_string()))?
            .to_string();
        let infra_type = record.get(1)
            .ok_or_else(|| ParserError::MissingColumn("infrastructure_type".to_string()))?
            .to_string();
        let is_domain = record.get(2)
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false);
        let is_damaged = record.get(3)
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false);
        
        if let Some(system) = systems.get_mut(&system_name) {
            let infrastructure = match infra_type.as_str() {
                "CommRelay" => Infrastructure::CommRelay { domain: is_domain },
                // The CSV uses Starsector's internal spelling "NavBouy"; accept
                // the corrected spelling too so a future data fix doesn't silently
                // drop the row into the `_ => continue` arm below.
                "NavBouy" | "NavBuoy" => Infrastructure::NavBuoy { domain: is_domain },
                "SensorArray" => Infrastructure::SensorArray { domain: is_domain },
                "Gate" => Infrastructure::Gate,
                "Remnants" => Infrastructure::Remnants { damaged: is_damaged },
                _ => continue, // Skip unknown infrastructure types
            };
            
            system.add_infrastructure(format!("{}-{}", system_name, infra_type), infrastructure);
        }
    }
    
    Ok(())
}

pub fn parse_systems_csv<P: AsRef<Path>>(
    path: P,
    systems: &mut HashMap<String, System>,
) -> Result<(), ParserError> {
    let mut rdr = csv::Reader::from_path(path)?;
    let headers = rdr.headers()?.clone();
    let stable_points_idx = headers
        .iter()
        .position(|h| h.eq_ignore_ascii_case("Stable Points"))
        .ok_or_else(|| ParserError::MissingColumn("Stable Points".to_string()))?;

    for result in rdr.records() {
        let record = result?;
        let system_name = record
            .get(0)
            .ok_or_else(|| ParserError::MissingColumn("System Name".to_string()))?;
        let stable_points = record
            .get(stable_points_idx)
            .unwrap_or("0")
            .trim()
            .parse::<u32>()
            .map_err(|_| {
                ParserError::InvalidValue(format!(
                    "invalid stable point count for system {system_name}"
                ))
            })?;

        if let Some(system) = systems.get_mut(system_name) {
            system.set_stable_points(stable_points);
        }
    }

    Ok(())
}

/// Insert a numeric property only when the column has a value, matching the
/// CSV parser's empty-cell-skip semantics.
fn insert_opt(properties: &mut FxHashMap<String, f64>, key: &str, value: Option<f64>) {
    if let Some(v) = value {
        properties.insert(key.to_string(), v);
    }
}

/// Insert a boolean property as 1.0/0.0. The exported CSVs write explicit
/// TRUE/FALSE cells which the CSV parser inserts as 1.0/0.0, so the key is
/// always present — replicate that exactly.
fn insert_bool(properties: &mut FxHashMap<String, f64>, key: &str, value: bool) {
    properties.insert(key.to_string(), if value { 1.0 } else { 0.0 });
}

/// Load solver game data from the save-extraction SQLite DB (see
/// SAVE_EXTRACTION_DESIGN.md). `save` selects the extracted save by
/// case-insensitive substring match on its directory or character name; `None`
/// picks the most recent one. Produces exactly what the CSV round trip
/// (`extract export` + `load_game_data`) would: same property keys and values,
/// same infrastructure variants, same stable-point counts.
pub fn load_game_data_from_db(
    db_path: impl AsRef<Path>,
    save: Option<&str>,
) -> Result<HashMap<String, System>, ParserError> {
    let db_path = db_path.as_ref();
    if !db_path.exists() {
        return Err(ParserError::InvalidValue(format!(
            "extraction DB {} not found — run `extract run` first (see `extract --help`)",
            db_path.display()
        )));
    }
    let db = Db::open(db_path)?;
    let saves = db.list_saves()?;
    if saves.is_empty() {
        return Err(ParserError::InvalidValue(format!(
            "no saves extracted into {} — run `extract run` first",
            db_path.display()
        )));
    }
    let chosen = match save {
        Some(needle) => {
            let lower = needle.to_lowercase();
            saves
                .iter()
                .find(|s| {
                    s.dir_name.to_lowercase().contains(&lower)
                        || s.character_name.to_lowercase().contains(&lower)
                })
                .ok_or_else(|| {
                    ParserError::InvalidValue(format!("no extracted save matching {needle:?}"))
                })?
        }
        None => &saves[0],
    };

    let save_filter = chosen.dir_name.to_lowercase();
    let mut systems: HashMap<String, System> = HashMap::new();

    for row in db.fetch_systems(Some(&save_filter), None)? {
        let planets = db.fetch_planets(row.id, &row.name)?;
        let infrastructure = db.fetch_infrastructure(row.id, &row.name)?;

        let system = systems
            .entry(row.name.clone())
            .or_insert_with(|| System::new(row.name.clone()));
        system.set_stable_points(row.stable_points);

        for planet in planets {
            let mut properties = FxHashMap::default();
            insert_opt(&mut properties, "ruins", planet.ruins);
            insert_opt(&mut properties, "farmland", planet.farmland);
            insert_opt(&mut properties, "rare ores", planet.rare_ores);
            insert_opt(&mut properties, "ores", planet.ores);
            insert_opt(&mut properties, "volatiles", planet.volatiles);
            insert_opt(&mut properties, "organics", planet.organics);
            insert_opt(&mut properties, "accessibility percent", planet.accessibility_percent);
            properties.insert("hazard percent".to_string(), planet.hazard_percent);
            insert_bool(&mut properties, "no atmosphere", planet.no_atmosphere);
            insert_bool(&mut properties, "very hot", planet.very_hot);
            insert_bool(&mut properties, "gas giant", planet.gas_giant);
            insert_bool(&mut properties, "habitable", planet.habitable);
            insert_bool(&mut properties, "extreme activity", planet.extreme_activity);
            insert_bool(&mut properties, "water", planet.water);

            system.add_planet(Planet::new(planet.name, properties));
        }

        for infra in infrastructure {
            let infrastructure = match infra.infrastructure_type.as_str() {
                "CommRelay" => Infrastructure::CommRelay { domain: infra.is_domain },
                "NavBouy" | "NavBuoy" => Infrastructure::NavBuoy { domain: infra.is_domain },
                "SensorArray" => Infrastructure::SensorArray { domain: infra.is_domain },
                "Gate" => Infrastructure::Gate,
                "Remnants" => Infrastructure::Remnants { damaged: infra.is_damaged },
                _ => continue, // Skip unknown infrastructure types (CoronalTap, Cryosleeper, ...)
            };
            system.add_infrastructure(
                format!("{}-{}", row.name, infra.infrastructure_type),
                infrastructure,
            );
        }
    }

    Ok(systems)
}

/// Helper function to load all data from CSV files
pub fn load_game_data<P: AsRef<Path>>(
    planets_path: P,
    systems_path: P,
    infrastructure_path: P
) -> Result<HashMap<String, System>, ParserError> {
    // Load planets and systems from the planets CSV
    let (_planets, mut systems) = parse_planets_csv(planets_path)?;

    parse_systems_csv(systems_path, &mut systems)?;
    
    // Add infrastructure
    parse_infrastructure_csv(infrastructure_path, &mut systems)?;
    
    Ok(systems)
}
