use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::constants::ColonyItem;
use crate::rank::RankScorer;
use crate::solver::{Balance, SolverSettings};

pub const CONFIG_PATH: &str = "workspace/solver_tui.toml";
pub const DEFAULT_DB_PATH: &str = "save_data.db";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
pub enum DiscoveryDefinition {
    AtLeastOneSurveyed,
    FullySurveyed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TuiConfig {
    pub credits: f64,
    pub story_points: u32,
    pub alpha_cores: u32,
    pub colony_items: BTreeMap<String, u32>,
    pub horizon_months: i32,
    pub solver_time_budget_ms: u32,
    pub discovery_definition: DiscoveryDefinition,
    pub include_core_worlds: bool,
    #[serde(default = "default_include_industry_upgrades")]
    pub include_industry_upgrades: bool,
    #[serde(default)]
    pub allow_parallel_builds: bool,
    #[serde(default = "default_rank_by_score_per_planet")]
    pub rank_by_score_per_planet: bool,
    #[serde(default = "default_rank_scorer")]
    pub rank_scorer: RankScorer,
    pub db_path: PathBuf,
    pub starsector_dir: Option<PathBuf>,
}

pub const fn default_rank_scorer() -> RankScorer {
    RankScorer::Bound
}

fn default_rank_by_score_per_planet() -> bool {
    true
}

fn default_include_industry_upgrades() -> bool {
    true
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            credits: 5_000_000.0,
            story_points: 5,
            alpha_cores: 1,
            colony_items: BTreeMap::new(),
            horizon_months: 120,
            solver_time_budget_ms: 25_000,
            discovery_definition: DiscoveryDefinition::AtLeastOneSurveyed,
            include_core_worlds: false,
            include_industry_upgrades: true,
            allow_parallel_builds: false,
            rank_by_score_per_planet: true,
            rank_scorer: default_rank_scorer(),
            db_path: PathBuf::from(DEFAULT_DB_PATH),
            starsector_dir: None,
        }
    }
}

impl TuiConfig {
    pub fn load(path: impl AsRef<Path>) -> (Self, Option<String>) {
        let path = path.as_ref();
        match fs::read_to_string(path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(config) => (config, None),
                Err(err) => (
                    Self::default(),
                    Some(format!("settings unreadable; using defaults ({err})")),
                ),
            },
            Err(err) if err.kind() == io::ErrorKind::NotFound => (Self::default(), None),
            Err(err) => (
                Self::default(),
                Some(format!("settings unreadable; using defaults ({err})")),
            ),
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("create settings dir: {err}"))?;
        }
        let encoded =
            toml::to_string_pretty(self).map_err(|err| format!("encode settings: {err}"))?;
        fs::write(path, encoded).map_err(|err| format!("write settings: {err}"))
    }

    pub fn balance(&self) -> Balance {
        let mut balance = Balance::new(self.credits, self.story_points, self.alpha_cores);
        for (name, count) in &self.colony_items {
            if let Some(item) = ColonyItem::from_str(name) {
                for _ in 0..*count {
                    balance.add_colony_item(item);
                }
            }
        }
        balance
    }

    pub fn solver_settings(&self) -> SolverSettings {
        SolverSettings {
            include_industry_upgrades: self.include_industry_upgrades,
            allow_parallel_builds: self.allow_parallel_builds,
        }
    }

    pub fn balance_signature(&self) -> BalanceSignature {
        BalanceSignature {
            credits_bits: self.credits.to_bits(),
            story_points: self.story_points,
            alpha_cores: self.alpha_cores,
            colony_items: self.colony_items.clone(),
            horizon_months: self.horizon_months,
            solver_time_budget_ms: self.solver_time_budget_ms,
            include_industry_upgrades: self.include_industry_upgrades,
            allow_parallel_builds: self.allow_parallel_builds,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalanceSignature {
    credits_bits: u64,
    story_points: u32,
    alpha_cores: u32,
    colony_items: BTreeMap<String, u32>,
    horizon_months: i32,
    solver_time_budget_ms: u32,
    include_industry_upgrades: bool,
    allow_parallel_builds: bool,
}
