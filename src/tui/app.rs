use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::extract::db::{InfraRowDb, PlanetRowDb, SaveRow, SystemDiscovery};
use crate::extract::model::SaveInfo;
use crate::rank::{peak_income, sort_rows_best_first, RankRow, RankScorer};
use crate::system::System;

use super::config::{BalanceSignature, DiscoveryDefinition, TuiConfig};
use super::jobs::{Job, JobEvent, JobOutput, JobRunner};

const AUTO_RANK_SCOPE_LIMIT: usize = 50;
const QUICK_SECONDS_PER_SYSTEM: f64 = 10.0;
const BOUND_SECONDS_PER_SYSTEM: f64 = 1.0;
const TEMPLATE_SECONDS_PER_SYSTEM: f64 = 0.15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Saves,
    Rank,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    Help,
    Settings,
    Scorer,
    SpoilerConfirm,
    QuitConfirm,
}

#[derive(Debug, Clone)]
pub struct SaveListRow {
    pub dir_name: String,
    pub character_name: String,
    pub modified: String,
    pub extracted_at: Option<String>,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PlanetDetail {
    pub row: PlanetRowDb,
    pub conditions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SystemDetail {
    pub name: String,
    pub planets: Vec<PlanetDetail>,
    pub infrastructure: Vec<InfraRowDb>,
}

#[derive(Debug, Clone)]
pub struct RankCache {
    pub save: String,
    pub scorer: RankScorer,
    pub rows: Vec<RankRow>,
    pub stale: bool,
    pub signature: BalanceSignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeMode {
    Discovered,
    All,
}

pub struct App {
    pub config: TuiConfig,
    pub active_screen: Screen,
    pub modal: Option<Modal>,
    pub saves: Vec<SaveListRow>,
    pub save_selection: usize,
    pub active_save: Option<SaveRow>,
    pub systems: HashMap<String, System>,
    pub discovery: Vec<SystemDiscovery>,
    pub scope_mode: ScopeMode,
    pub spoiler_confirmed: bool,
    pub scorer: RankScorer,
    pub scorer_picker_original: Option<RankScorer>,
    pub rank_rows: Vec<RankRow>,
    pub rank_selection: usize,
    pub selected_system_name: Option<String>,
    pub system_details: HashMap<String, SystemDetail>,
    pub system_planet_selection: usize,
    pub rank_cache: Vec<RankCache>,
    pub rank_filter: String,
    pub editing_filter: bool,
    pub status: String,
    pub error: Option<String>,
    pub job: JobRunner,
    pub should_quit: bool,
    pub spinner: usize,
    pub settings_selection: usize,
    pub settings_editing: bool,
    pub settings_input: String,
    first_rank_entry: bool,
}

impl App {
    pub fn new(config: TuiConfig, status: Option<String>) -> Self {
        Self {
            config,
            active_screen: Screen::Saves,
            modal: None,
            saves: Vec::new(),
            save_selection: 0,
            active_save: None,
            systems: HashMap::new(),
            discovery: Vec::new(),
            scope_mode: ScopeMode::Discovered,
            spoiler_confirmed: false,
            scorer: RankScorer::Quick,
            scorer_picker_original: None,
            rank_rows: Vec::new(),
            rank_selection: 0,
            selected_system_name: None,
            system_details: HashMap::new(),
            system_planet_selection: 0,
            rank_cache: Vec::new(),
            rank_filter: String::new(),
            editing_filter: false,
            status: status.unwrap_or_else(|| "ready".to_string()),
            error: None,
            job: JobRunner::new(),
            should_quit: false,
            spinner: 0,
            settings_selection: 0,
            settings_editing: false,
            settings_input: String::new(),
            first_rank_entry: true,
        }
    }

    pub fn start_initial_load(&mut self) {
        self.start_job(Job::LoadSaves {
            db_path: self.config.db_path.clone(),
            starsector_dir: self.config.starsector_dir.clone(),
        });
    }

    pub fn start_job(&mut self, job: Job) {
        if self.job.is_running() {
            self.status = "a job is running - wait or cancel".to_string();
            return;
        }
        self.status = job.label().to_string();
        self.job.start(job);
    }

    pub fn cancel_job(&mut self) {
        if let Some(label) = self.job.detach() {
            self.status = format!("{label} detached; compute continues in background");
        }
    }

    pub fn tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
        for event in self.job.drain() {
            self.apply_event(event);
        }
    }

    fn apply_event(&mut self, event: JobEvent) {
        match event {
            JobEvent::Progress(text) => self.status = text,
            JobEvent::RankRow(row) => {
                self.push_rank_row(row);
            }
            JobEvent::Done(output) => {
                self.job.clear();
                self.apply_output(output);
            }
            JobEvent::Failed(err) => {
                self.job.clear();
                self.error = Some(err.clone());
                self.status = err;
            }
        }
    }

    fn apply_output(&mut self, output: JobOutput) {
        match output {
            JobOutput::Saves {
                saves,
                extracted,
                default_active,
            } => {
                self.saves = merge_saves(saves, &extracted);
                self.error = None;
                if let Some(save) = default_active {
                    self.active_save = Some(save.clone());
                    self.status = format!("active save: {}", save.dir_name);
                    self.active_screen = Screen::Rank;
                    self.start_load_systems(save.dir_name);
                } else {
                    self.status = format!("loaded {} save rows", self.saves.len());
                }
            }
            JobOutput::Extracted(save) => {
                self.active_save = Some(save.clone());
                self.active_screen = Screen::Rank;
                self.status = format!("extracted {}", save.dir_name);
                self.start_load_systems(save.dir_name);
            }
            JobOutput::Systems {
                systems,
                discovery,
                details,
            } => {
                self.systems = systems;
                self.discovery = discovery;
                self.system_details = details;
                self.rank_rows = self.cached_rows_for_active().unwrap_or_default();
                self.status = format!("loaded {} systems", self.systems.len());
                self.maybe_auto_rank();
            }
            JobOutput::RankComplete(rows) => {
                self.rank_rows = rows;
                let save = self
                    .active_save
                    .as_ref()
                    .map(|s| s.dir_name.clone())
                    .unwrap_or_default();
                self.rank_cache.retain(|cache| {
                    !(cache.save == save
                        && cache.scorer == self.scorer
                        && cache.signature == self.config.balance_signature())
                });
                self.rank_cache.push(RankCache {
                    save,
                    scorer: self.scorer,
                    rows: self.rank_rows.clone(),
                    stale: false,
                    signature: self.config.balance_signature(),
                });
                self.status = format!("ranked {} systems", self.rank_rows.len());
            }
        }
    }

    fn start_load_systems(&mut self, save: String) {
        self.start_job(Job::LoadSystems {
            db_path: self.config.db_path.clone(),
            save,
        });
    }

    pub fn start_rank(&mut self) {
        let names = self.visible_scope_names();
        if names.is_empty() {
            self.status = "no systems in current scope".to_string();
            return;
        }
        self.rank_rows.clear();
        self.rank_selection = 0;
        self.selected_system_name = None;
        self.start_job(Job::Rank {
            systems: self.systems.clone(),
            names,
            balance: self.config.balance(),
            horizon: self.config.horizon_months,
            time_limit: self.config.solver_time_budget_ms,
            scorer: self.scorer,
        });
    }

    pub fn maybe_auto_rank(&mut self) {
        if !self.first_rank_entry || !self.rank_rows.is_empty() || self.job.is_running() {
            return;
        }
        self.first_rank_entry = false;
        let count = self.visible_scope_names().len();
        if count <= AUTO_RANK_SCOPE_LIMIT {
            self.start_rank();
        } else {
            self.status = format!(
                "press r to rank ~{} systems (~{})",
                count,
                format_duration(estimate_rank_cost(count, self.scorer))
            );
        }
    }

    pub fn visible_scope_names(&self) -> Vec<String> {
        filter_scope(&self.discovery, self.config.discovery_definition, self.config.include_core_worlds, self.scope_mode)
            .into_iter()
            .filter(|name| self.systems.contains_key(*name))
            .cloned()
            .collect()
    }

    pub fn visible_rank_rows(&self) -> Vec<RankRow> {
        let scope = self.visible_scope_names();
        let filter = self.rank_filter.to_lowercase();
        let mut rows: Vec<RankRow> = self
            .rank_rows
            .iter()
            .filter(|row| scope.iter().any(|name| name == &row.system))
            .filter(|row| filter.is_empty() || row.system.to_lowercase().contains(&filter))
            .cloned()
            .collect();
        sort_rows_best_first(&mut rows);
        rows
    }

    fn push_rank_row(&mut self, row: RankRow) {
        let selected = self
            .visible_rank_rows()
            .get(self.rank_selection)
            .map(|r| r.system.clone())
            .or_else(|| self.selected_system_name.clone());
        self.rank_rows.retain(|existing| existing.system != row.system);
        self.rank_rows.push(row);
        sort_rows_best_first(&mut self.rank_rows);
        if let Some(name) = selected {
            let rows = self.visible_rank_rows();
            if let Some(index) = rows.iter().position(|row| row.system == name) {
                self.rank_selection = index;
                self.selected_system_name = Some(name);
                return;
            }
        }
        self.rank_selection = self.rank_selection.min(self.visible_rank_rows().len().saturating_sub(1));
    }

    fn cached_rows_for_active(&self) -> Option<Vec<RankRow>> {
        let save = self.active_save.as_ref()?.dir_name.as_str();
        self.rank_cache
            .iter()
            .find(|cache| {
                cache.save == save
                    && cache.scorer == self.scorer
                    && cache.signature == self.config.balance_signature()
            })
            .map(|cache| cache.rows.clone())
    }

    pub fn move_scorer_picker(&mut self, delta: i32) {
        let scorers = [RankScorer::Quick, RankScorer::Template, RankScorer::Bound];
        let current = scorers
            .iter()
            .position(|scorer| *scorer == self.scorer)
            .unwrap_or(0);
        let next = if delta < 0 {
            current.saturating_sub(1)
        } else {
            (current + 1).min(scorers.len().saturating_sub(1))
        };
        self.scorer = scorers[next];
    }

    pub fn close_scorer_picker(&mut self) {
        let original = self.scorer_picker_original.take();
        self.modal = None;
        if original == Some(self.scorer) {
            return;
        }

        let previously_selected = self
            .visible_rank_rows()
            .get(self.rank_selection)
            .map(|row| row.system.clone())
            .or_else(|| self.selected_system_name.clone());
        if let Some(rows) = self.cached_rows_for_active() {
            self.rank_rows = rows;
            let visible = self.visible_rank_rows();
            self.rank_selection = previously_selected
                .and_then(|name| visible.iter().position(|row| row.system == name))
                .unwrap_or(0);
        }
        self.status = format!("scorer: {} - r to re-rank", scorer_name(self.scorer));
    }

    pub fn mark_rank_stale(&mut self) {
        for cache in &mut self.rank_cache {
            cache.stale = true;
        }
        self.status = "scores are stale (balance changed) - r to re-rank".to_string();
    }

    pub fn open_selected_system(&mut self) {
        let rows = self.visible_rank_rows();
        if let Some(row) = rows.get(self.rank_selection) {
            self.selected_system_name = Some(row.system.clone());
            self.system_planet_selection = 0;
            self.active_screen = Screen::System;
            self.status = format!("system: {}", row.system);
        }
    }

    pub fn export_rank_csv(&mut self) {
        let rows = self.visible_rank_rows();
        let mut out = String::from("system,score,peak_income,seconds\n");
        for row in &rows {
            out.push_str(&format!(
                "{},{:.3},{:.0},{:.2}\n",
                row.system,
                row.solve.score,
                peak_income(&row.solve),
                row.seconds
            ));
        }
        match std::fs::write("rank_tui.csv", out) {
            Ok(()) => self.status = "exported rank_tui.csv".to_string(),
            Err(err) => self.status = format!("export failed: {err}"),
        }
    }

    pub fn active_save_label(&self) -> String {
        self.active_save
            .as_ref()
            .map(|s| format!("{} / {} ({})", s.dir_name, s.character_name, s.extracted_at))
            .unwrap_or_else(|| "none".to_string())
    }

    pub fn elapsed_job(&self) -> Option<Duration> {
        self.job.started_at().map(|started| started.elapsed())
    }
}

pub fn merge_saves(disk: Vec<SaveInfo>, extracted: &[SaveRow]) -> Vec<SaveListRow> {
    let mut rows = Vec::new();
    for save in disk {
        let match_row = extracted.iter().find(|row| row.dir_name == save.dir_name);
        rows.push(SaveListRow {
            dir_name: save.dir_name,
            character_name: save.character_name,
            modified: format_system_time(save.modified),
            extracted_at: match_row.map(|row| row.extracted_at.clone()),
            path: save.path,
        });
    }
    for row in extracted {
        if rows.iter().any(|existing| existing.dir_name == row.dir_name) {
            continue;
        }
        rows.push(SaveListRow {
            dir_name: row.dir_name.clone(),
            character_name: row.character_name.clone(),
            modified: row.save_date.clone(),
            extracted_at: Some(row.extracted_at.clone()),
            path: PathBuf::from(&row.path),
        });
    }
    rows
}

pub fn filter_scope(
    rows: &[SystemDiscovery],
    definition: DiscoveryDefinition,
    include_core_worlds: bool,
    mode: ScopeMode,
) -> Vec<&String> {
    rows.iter()
        .filter(|row| include_core_worlds || !row.is_core)
        .filter(|row| match mode {
            ScopeMode::All => true,
            ScopeMode::Discovered => match definition {
                DiscoveryDefinition::AtLeastOneSurveyed => row.surveyed_any >= 1,
                DiscoveryDefinition::FullySurveyed => {
                    row.planet_count > 0 && row.surveyed_full == row.planet_count
                }
            },
        })
        .map(|row| &row.system_name)
        .collect()
}

pub fn estimate_rank_cost(system_count: usize, scorer: RankScorer) -> Duration {
    let seconds = match scorer {
        RankScorer::Quick => QUICK_SECONDS_PER_SYSTEM,
        RankScorer::Bound => BOUND_SECONDS_PER_SYSTEM,
        RankScorer::Template => TEMPLATE_SECONDS_PER_SYSTEM,
    };
    Duration::from_secs_f64(system_count as f64 * seconds)
}

pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}

pub(crate) fn format_system_time(time: std::time::SystemTime) -> String {
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => {
            let days = (duration.as_secs() / 86_400) as i64;
            let (year, month, day) = civil_from_days(days);
            format!("{year:04}-{month:02}-{day:02}")
        }
        Err(_) => "-".to_string(),
    }
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

fn scorer_name(scorer: RankScorer) -> &'static str {
    match scorer {
        RankScorer::Quick => "quick",
        RankScorer::Template => "template",
        RankScorer::Bound => "bound",
    }
}
