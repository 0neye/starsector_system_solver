//! Shared data structs for the extraction layer. These are the fixed interfaces
//! between save.rs/scan.rs (producers) and mapping.rs/db.rs (consumers).
//! See SAVE_EXTRACTION_DESIGN.md.

use std::path::PathBuf;
use std::time::SystemTime;

/// One save directory, parsed from `descriptor.xml` (cheap; no campaign.xml access).
#[derive(Debug, Clone)]
pub struct SaveInfo {
    /// Directory name, e.g. `save_DEMIURGE_8035172239347920742`.
    pub dir_name: String,
    /// Absolute path to the save directory.
    pub path: PathBuf,
    pub character_name: String,
    /// Raw string from descriptor, e.g. `2026-05-19 06:06:55.231 UTC`.
    pub save_date: String,
    pub game_version: String,
    pub character_level: u32,
    pub compressed: bool,
    /// Filesystem mtime of campaign.xml (fallback ordering).
    pub modified: SystemTime,
}

/// A custom campaign entity (CCEnt) relevant to extraction.
#[derive(Debug, Clone)]
pub struct RawEntity {
    /// Spec id from j0.f3, e.g. `comm_relay_makeshift`, `stable_location`.
    pub spec_id: String,
    /// Display name from j0.f0, if present.
    pub name: Option<String>,
}

/// A planet (not a star) extracted from the save.
#[derive(Debug, Clone)]
pub struct RawPlanet {
    /// Display name from j0.f0.
    pub name: String,
    /// Entity id from j0.f4, if present.
    pub internal_id: Option<String>,
    /// `<type>` id, e.g. `gas_giant`, `US_barrenE`.
    pub planet_type: String,
    pub radius: f64,
    /// `<tags>` entries, e.g. `planet`, `gas_giant`.
    pub tags: Vec<String>,
    /// Market condition ids in save order.
    pub conditions: Vec<String>,
    /// `<surveyLevel>` if present (NONE/SEEN/PRELIMINARY/FULL).
    pub survey_level: Option<String>,
    /// Owner faction id if the market is colonized (e.g. `hegemony`, `player`).
    pub owner_faction: Option<String>,
    /// Market size (0 for uncolonized PCMarket).
    pub market_size: u32,
    /// True when this planet orbits another planet.
    pub is_moon: bool,
}

/// A star system extracted from the save.
#[derive(Debug, Clone)]
pub struct RawSystem {
    /// `bN` attribute, e.g. `Elgava` (matches CSV "System Name" style).
    pub name: String,
    /// `dN` attribute, e.g. `Elgava Star System`.
    pub display_name: String,
    /// Internal id (j0.f4 of the system, or the z attr as string fallback).
    pub internal_id: String,
    /// Hyperspace position in raw units (from the hyperspace anchor's `<loc>`).
    pub hyper_loc: Option<(f64, f64)>,
    /// System-level tags (theme_remnant_*, theme_core_populated, ...).
    pub tags: Vec<String>,
    /// `<type>` ids of star-tagged Plnt entities in this system.
    pub star_types: Vec<String>,
    pub planets: Vec<RawPlanet>,
    /// Relevant CCEnt entities (objectives, stable points, gates, taps, ...).
    pub entities: Vec<RawEntity>,
}

/// Everything extracted from one campaign.xml.
#[derive(Debug, Clone)]
pub struct RawSave {
    pub systems: Vec<RawSystem>,
}

// ---------------------------------------------------------------------------
// Mapped rows (produced by mapping.rs, consumed by db.rs)
// ---------------------------------------------------------------------------

/// Mirrors a Systems.csv row plus extras.
#[derive(Debug, Clone)]
pub struct SystemRow {
    pub name: String,
    pub display_name: String,
    pub internal_id: String,
    pub x_ly: Option<f64>,
    pub y_ly: Option<f64>,
    pub dist_from_com_ly: Option<f64>,
    pub stable_points: u32,
    pub has_gate: bool,
    pub has_remnants: bool,
    pub remnant_damaged: bool,
    pub star_types: Vec<String>,
    pub tags: Vec<String>,
}

/// Mirrors a Planets.csv row plus extras. `None` resource = empty CSV cell.
#[derive(Debug, Clone)]
pub struct PlanetRow {
    pub name: String,
    pub internal_id: Option<String>,
    pub planet_type: String,
    /// Equal to planet_type for vanilla types; closest vanilla type for modded ones.
    pub mapped_vanilla_type: Option<String>,
    pub is_moon: bool,
    pub survey_level: Option<String>,
    pub owner_faction: Option<String>,
    pub radius: f64,
    pub ruins: Option<f64>,
    pub farmland: Option<f64>,
    pub rare_ores: Option<f64>,
    pub ores: Option<f64>,
    pub volatiles: Option<f64>,
    pub organics: Option<f64>,
    pub accessibility_percent: Option<f64>,
    pub hazard_percent: f64,
    /// True when at least one condition had no known hazard value.
    pub hazard_incomplete: bool,
    pub no_atmosphere: bool,
    pub very_hot: bool,
    pub gas_giant: bool,
    pub habitable: bool,
    pub extreme_activity: bool,
    pub water: bool,
    /// Full-fidelity condition list for the planet_conditions table.
    pub conditions: Vec<String>,
}

/// Mirrors an Infrastructure.csv row.
#[derive(Debug, Clone)]
pub struct InfraRow {
    pub infrastructure_type: String, // CommRelay | NavBouy | SensorArray | Gate | Remnants | CoronalTap | Cryosleeper
    pub is_domain: bool,
    pub is_damaged: bool,
}

/// One mapped system ready for DB insertion.
#[derive(Debug, Clone)]
pub struct MappedSystem {
    pub system: SystemRow,
    pub planets: Vec<PlanetRow>,
    pub infrastructure: Vec<InfraRow>,
}

/// A condition id seen in the save with no known spec.
#[derive(Debug, Clone)]
pub struct UnknownCondition {
    pub condition_id: String,
    pub occurrences: u32,
    pub example_planet: String,
}

/// Full mapping result for one save, ready for DB insertion.
#[derive(Debug, Clone)]
pub struct MappedOutput {
    pub systems: Vec<MappedSystem>,
    pub unknown_conditions: Vec<UnknownCondition>,
    pub type_mappings: Vec<TypeMapping>,
    /// Planet type ids found in the save but in no planets.json (vanilla or mod).
    pub unknown_types: Vec<String>,
}

/// Similarity of a modded planet type to a vanilla one.
#[derive(Debug, Clone)]
pub struct TypeMapping {
    pub modded_type: String,
    pub vanilla_type: String,
    pub similarity: f64,
    pub modded_samples: u32,
    pub vanilla_samples: u32,
}
