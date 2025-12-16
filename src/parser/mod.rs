use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::path::Path;

use csv;
use rustc_hash::FxHashMap;

use crate::planet::Planet;
use crate::system::{System, Infrastructure};
use crate::constants::AdminType;

#[derive(Debug)]
pub enum ParserError {
    MissingColumn(String),
    InvalidValue(String),
    IoError(std::io::Error),
    CsvError(csv::Error),
}

impl std::fmt::Display for ParserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParserError::MissingColumn(col) => write!(f, "Missing column: {}", col),
            ParserError::InvalidValue(msg) => write!(f, "Invalid value: {}", msg),
            ParserError::IoError(e) => write!(f, "IO error: {}", e),
            ParserError::CsvError(e) => write!(f, "CSV error: {}", e),
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
                "NavBuoy" => Infrastructure::NavBuoy { domain: is_domain },
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

/// Helper function to load all data from CSV files
pub fn load_game_data<P: AsRef<Path>>(
    planets_path: P,
    infrastructure_path: P
) -> Result<HashMap<String, System>, ParserError> {
    // Load planets and systems from the planets CSV
    let (_planets, mut systems) = parse_planets_csv(planets_path)?;
    
    // Add infrastructure
    parse_infrastructure_csv(infrastructure_path, &mut systems)?;
    
    Ok(systems)
}
