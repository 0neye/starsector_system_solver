use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::extract::db::{InfraRowDb, PlanetRowDb, SaveRow, SystemDiscovery};
use crate::extract::model::SaveInfo;
use crate::rank::{peak_income, sort_rows_best_first, RankRow, RankScorer, RankSortMode};
use crate::solve::SolveOutcome;
use crate::solver::pareto::{FrontierKind, ParetoPoint, ParetoSolve};
use crate::solver::{Action, Metric};
use crate::system::System;

use super::config::{default_rank_scorer, BalanceSignature, DiscoveryDefinition, TuiConfig};
use super::jobs::{Job, JobEvent, JobOutput, JobRunner};

const AUTO_RANK_SCOPE_LIMIT: usize = 50;
const QUICK_SECONDS_PER_SYSTEM: f64 = 10.0;
const BOUND_SECONDS_PER_SYSTEM: f64 = 1.0;
const TEMPLATE_SECONDS_PER_SYSTEM: f64 = 0.15;
pub const DEFAULT_TUI_RANK_SCORER: RankScorer = default_rank_scorer();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Saves,
    Rank,
    System,
    Solve,
    Plan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    Help,
    Settings,
    Scorer,
    RankSort,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolveMode {
    Pareto,
    Goal,
    Maximize,
}

impl SolveMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SolveMode::Pareto => "Pareto",
            SolveMode::Goal => "Goal",
            SolveMode::Maximize => "Maximize",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolveFocus {
    Parameters,
    Results,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SolveParams {
    pub mode: SolveMode,
    pub goal_income: f64,
    pub goal_stability: i32,
    pub goal_defense: f64,
    pub maximize_metric: Metric,
    pub floor_income: f64,
    pub floor_stability: i32,
    pub floor_defense: f64,
    pub horizon: i32,
    pub time_limit: u32,
}

impl SolveParams {
    pub fn from_config(config: &TuiConfig) -> Self {
        Self {
            mode: SolveMode::Pareto,
            goal_income: 200_000.0,
            goal_stability: 8,
            goal_defense: 0.0,
            maximize_metric: Metric::Income,
            floor_income: 0.0,
            floor_stability: 0,
            floor_defense: 0.0,
            horizon: config.horizon_months,
            time_limit: config.solver_time_budget_ms,
        }
    }
}

#[derive(Debug, Clone)]
pub enum SolveResult {
    Pareto(ParetoSolve),
    Goal(Option<SolveOutcome>),
    Maximize(Option<SolveOutcome>),
}

#[derive(Debug, Clone)]
pub struct SolveCacheEntry {
    pub key: String,
    pub result: SolveResult,
}

#[derive(Debug, Clone)]
pub struct PlanActionRow {
    pub month: i32,
    pub action_index: usize,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct PlanState {
    pub header: String,
    pub actions: Vec<Action>,
    pub rows: Vec<PlanActionRow>,
    pub checked: Vec<bool>,
    pub selection: usize,
}

pub struct App {
    pub config: TuiConfig,
    config_path: PathBuf,
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
    pub rank_sort_picker_original: Option<bool>,
    pub rank_sort_picker_selected: Option<String>,
    pub rank_rows: Vec<RankRow>,
    pub rank_selection: usize,
    pub selected_system_name: Option<String>,
    pub system_details: HashMap<String, SystemDetail>,
    pub system_planet_selection: usize,
    pub rank_cache: Vec<RankCache>,
    pub rank_filter: String,
    pub editing_filter: bool,
    /// The displayed rank rows were scored with a different balance/settings
    /// than the current ones (shown in the Rank header until re-ranked).
    pub rank_rows_stale: bool,
    pub status: String,
    pub error: Option<String>,
    pub job: JobRunner,
    pub should_quit: bool,
    pub spinner: usize,
    pub settings_selection: usize,
    pub settings_editing: bool,
    pub settings_input: String,
    pub solve_params: SolveParams,
    pub solve_focus: SolveFocus,
    pub solve_param_selection: usize,
    pub solve_result_selection: usize,
    pub solve_result: Option<SolveResult>,
    pub solve_cache: Vec<SolveCacheEntry>,
    pub editing_solve_param: bool,
    pub solve_input: String,
    pub plan: Option<PlanState>,
    first_rank_entry: bool,
}

impl App {
    pub fn new(config: TuiConfig, status: Option<String>) -> Self {
        let solve_params = SolveParams::from_config(&config);
        let scorer = config.rank_scorer;
        Self {
            config,
            config_path: PathBuf::from(super::config::CONFIG_PATH),
            active_screen: Screen::Saves,
            modal: None,
            saves: Vec::new(),
            save_selection: 0,
            active_save: None,
            systems: HashMap::new(),
            discovery: Vec::new(),
            scope_mode: ScopeMode::Discovered,
            spoiler_confirmed: false,
            scorer,
            scorer_picker_original: None,
            rank_sort_picker_original: None,
            rank_sort_picker_selected: None,
            rank_rows: Vec::new(),
            rank_selection: 0,
            selected_system_name: None,
            system_details: HashMap::new(),
            system_planet_selection: 0,
            rank_cache: Vec::new(),
            rank_filter: String::new(),
            editing_filter: false,
            rank_rows_stale: false,
            status: status.unwrap_or_else(|| "ready".to_string()),
            error: None,
            job: JobRunner::new(),
            should_quit: false,
            spinner: 0,
            settings_selection: 0,
            settings_editing: false,
            settings_input: String::new(),
            solve_params,
            solve_focus: SolveFocus::Parameters,
            solve_param_selection: 0,
            solve_result_selection: 0,
            solve_result: None,
            solve_cache: Vec::new(),
            editing_solve_param: false,
            solve_input: String::new(),
            plan: None,
            first_rank_entry: true,
        }
    }

    pub fn start_initial_load(&mut self) {
        self.start_job(Job::LoadSaves {
            db_path: self.config.db_path.clone(),
            starsector_dir: self.config.starsector_dir.clone(),
        });
    }

    #[cfg(test)]
    pub(crate) fn set_config_path_for_test(&mut self, path: PathBuf) {
        self.config_path = path;
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
        if self.job.request_cancel() {
            self.status = "cancelling - solver stops at the next checkpoint".to_string();
        } else if let Some(label) = self.job.detach() {
            // IO-bound jobs (extract/load) have no cancel points; detach only.
            self.status = format!("{label} detached; compute continues in background");
        }
    }

    /// v1.5 balance-from-save: overwrite the Setup balance with the save's
    /// credits/story points/alpha cores/colony items when a save's systems
    /// are loaded. Still freely editable in Setup afterwards; persisted so a
    /// restart keeps whatever the user last saw. Returns true if anything
    /// changed.
    fn seed_balance_from_save(&mut self, balance: crate::extract::model::PlayerBalance) -> bool {
        use crate::constants::ColonyItem;
        let mut items = std::collections::BTreeMap::new();
        for (item_id, count) in &balance.items {
            if let Some(item) = ColonyItem::from_save_id(item_id) {
                *items.entry(item.name().to_string()).or_insert(0) += *count;
            }
        }
        let changed = self.config.credits != balance.credits
            || self.config.story_points != balance.story_points
            || self.config.alpha_cores != balance.alpha_cores
            || self.config.colony_items != items;
        if changed {
            self.config.credits = balance.credits;
            self.config.story_points = balance.story_points;
            self.config.alpha_cores = balance.alpha_cores;
            self.config.colony_items = items;
            let _ = self.config.save(&self.config_path);
        }
        changed
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
                let cancelled = self.job.is_cancelling();
                self.job.clear();
                if cancelled {
                    // Partial result from a cancelled solve/rank: discard so
                    // caches never hold truncated results.
                    self.status = "cancelled".to_string();
                } else {
                    self.apply_output(output);
                }
            }
            JobEvent::Failed(err) => {
                let cancelled = self.job.is_cancelling();
                self.job.clear();
                if cancelled {
                    self.status = "cancelled".to_string();
                } else {
                    self.error = Some(err.clone());
                    self.status = err;
                }
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
                player_balance,
            } => {
                self.systems = systems;
                self.discovery = discovery;
                self.system_details = details;
                let seeded = player_balance
                    .map(|balance| self.seed_balance_from_save(balance))
                    .unwrap_or(false);
                self.rank_rows = self.cached_rows_for_active().unwrap_or_default();
                self.status = if seeded {
                    format!(
                        "loaded {} systems · balance seeded from save (editable in Setup)",
                        self.systems.len()
                    )
                } else {
                    format!("loaded {} systems", self.systems.len())
                };
                self.maybe_auto_rank();
            }
            JobOutput::RankComplete(rows) => {
                self.rank_rows = rows;
                self.rank_rows_stale = false;
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
            JobOutput::SolveComplete { key, result } => {
                self.solve_result = Some(result.clone());
                self.solve_result_selection = 0;
                self.solve_cache.retain(|entry| entry.key != key);
                self.solve_cache.push(SolveCacheEntry { key, result });
                self.status = "solve complete".to_string();
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
        self.rank_rows_stale = false;
        self.rank_selection = 0;
        self.selected_system_name = None;
        self.start_job(Job::Rank {
            systems: self.systems.clone(),
            names,
            balance: self.config.balance(),
            horizon: self.config.horizon_months,
            time_limit: self.config.solver_time_budget_ms,
            scorer: self.scorer,
            include_industry_upgrades: self.config.include_industry_upgrades,
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
        filter_scope(
            &self.discovery,
            self.config.discovery_definition,
            self.config.include_core_worlds,
            self.scope_mode,
        )
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
        sort_rows_best_first(&mut rows, self.rank_sort_mode());
        rows
    }

    fn push_rank_row(&mut self, row: RankRow) {
        let selected = self
            .visible_rank_rows()
            .get(self.rank_selection)
            .map(|r| r.system.clone())
            .or_else(|| self.selected_system_name.clone());
        self.rank_rows
            .retain(|existing| existing.system != row.system);
        self.rank_rows.push(row);
        let mode = self.rank_sort_mode();
        sort_rows_best_first(&mut self.rank_rows, mode);
        if let Some(name) = selected {
            let rows = self.visible_rank_rows();
            if let Some(index) = rows.iter().position(|row| row.system == name) {
                self.rank_selection = index;
                self.selected_system_name = Some(name);
                return;
            }
        }
        self.rank_selection = self
            .rank_selection
            .min(self.visible_rank_rows().len().saturating_sub(1));
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
            // A signature-matching cache hit is fresh by definition.
            self.rank_rows = rows;
            self.rank_rows_stale = false;
            let visible = self.visible_rank_rows();
            self.rank_selection = previously_selected
                .and_then(|name| visible.iter().position(|row| row.system == name))
                .unwrap_or(0);
        }
        self.config.rank_scorer = self.scorer;
        match self.config.save(&self.config_path) {
            Ok(()) => {
                self.status = format!("scorer: {} - r to re-rank", scorer_name(self.scorer));
            }
            Err(err) => self.status = err,
        }
    }

    pub fn open_rank_sort_picker(&mut self) {
        self.rank_sort_picker_original = Some(self.config.rank_by_score_per_planet);
        self.rank_sort_picker_selected = self
            .visible_rank_rows()
            .get(self.rank_selection)
            .map(|row| row.system.clone());
        self.modal = Some(Modal::RankSort);
    }

    pub fn move_rank_sort_picker(&mut self, delta: i32) {
        let modes = [true, false];
        let current = modes
            .iter()
            .position(|mode| *mode == self.config.rank_by_score_per_planet)
            .unwrap_or(0);
        let next = if delta < 0 {
            current.saturating_sub(1)
        } else {
            (current + 1).min(modes.len().saturating_sub(1))
        };
        self.config.rank_by_score_per_planet = modes[next];
        self.restore_rank_sort_picker_selection();
    }

    pub fn close_rank_sort_picker(&mut self) {
        let original = self.rank_sort_picker_original.take();
        self.rank_sort_picker_selected = None;
        self.modal = None;
        if original == Some(self.config.rank_by_score_per_planet) {
            return;
        }

        match self.config.save(&self.config_path) {
            Ok(()) => {
                self.status = format!("rank sort: {}", rank_sort_name(self.rank_sort_mode()));
            }
            Err(err) => self.status = err,
        }
    }

    pub fn cancel_rank_sort_picker(&mut self) {
        if let Some(original) = self.rank_sort_picker_original {
            self.config.rank_by_score_per_planet = original;
            self.restore_rank_sort_picker_selection();
        }
        self.close_rank_sort_picker();
    }

    pub fn restore_rank_sort_picker_selection(&mut self) {
        let Some(selected) = self.rank_sort_picker_selected.clone() else {
            return;
        };
        if let Some(index) = self
            .visible_rank_rows()
            .iter()
            .position(|row| row.system == selected)
        {
            self.rank_selection = index;
        }
    }

    pub fn mark_rank_stale(&mut self) {
        for cache in &mut self.rank_cache {
            cache.stale = true;
        }
        self.rank_rows_stale = !self.rank_rows.is_empty();
        self.status = "scores are stale (balance changed) - r to re-rank".to_string();
    }

    pub fn rank_sort_mode(&self) -> RankSortMode {
        if self.config.rank_by_score_per_planet {
            RankSortMode::ScorePerPlanet
        } else {
            RankSortMode::TotalScore
        }
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

    pub fn open_solve_for_selected_system(&mut self) {
        if self.selected_system_name.is_none() {
            self.status = "open a system first".to_string();
            return;
        }
        self.active_screen = Screen::Solve;
        self.restore_solve_cache();
    }

    pub fn current_system(&self) -> Option<&System> {
        let name = self.selected_system_name.as_ref()?;
        self.systems.get(name)
    }

    pub fn planet_name_map_for_selected(&self) -> HashMap<u64, String> {
        self.current_system()
            .map(|system| {
                system
                    .planets()
                    .iter()
                    .map(|(hash, planet)| (*hash, planet.name().to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn solve_cache_key(&self) -> Option<String> {
        Some(solve_cache_key(
            self.selected_system_name.as_ref()?,
            &self.solve_params,
            &self.config.balance_signature(),
        ))
    }

    pub fn restore_solve_cache(&mut self) {
        if let Some(key) = self.solve_cache_key() {
            self.solve_result = self
                .solve_cache
                .iter()
                .find(|entry| entry.key == key)
                .map(|entry| entry.result.clone());
            self.solve_result_selection = 0;
        }
    }

    pub fn start_solve(&mut self) {
        let Some(system_name) = self.selected_system_name.clone() else {
            self.status = "open a system first".to_string();
            return;
        };
        let Some(system) = self.systems.get(&system_name).cloned() else {
            self.status = "selected system is not loaded".to_string();
            return;
        };
        let Some(key) = self.solve_cache_key() else {
            self.status = "open a system first".to_string();
            return;
        };
        self.start_job(Job::Solve {
            key,
            system_name,
            system,
            balance: self.config.balance(),
            params: self.solve_params.clone(),
            include_industry_upgrades: self.config.include_industry_upgrades,
        });
    }

    pub fn open_plan(&mut self) {
        let Some((header, actions)) = self.selected_plan_actions() else {
            self.status = "no plan to open".to_string();
            return;
        };
        let planet_names = self.planet_name_map_for_selected();
        let rows = group_plan_actions(&actions, &planet_names);
        self.plan = Some(PlanState {
            header,
            checked: vec![false; rows.len()],
            actions,
            rows,
            selection: 0,
        });
        self.active_screen = Screen::Plan;
    }

    fn selected_plan_actions(&self) -> Option<(String, Vec<Action>)> {
        let system_name = self.selected_system_name.as_ref()?;
        match self.solve_result.as_ref()? {
            SolveResult::Pareto(solve) => {
                let points = pareto_points(solve);
                let point = points.get(self.solve_result_selection)?;
                Some((
                    format!(
                        "{} · {} floor {:.0} -> income {:.0}, stability {:.1}, defense {:.1}, month {}",
                        system_name,
                        point.kind.as_str(),
                        point.floor,
                        point.income,
                        point.stability,
                        point.defense,
                        point.months
                    ),
                    point.actions.clone(),
                ))
            }
            SolveResult::Goal(Some(outcome)) => Some((
                format!(
                    "{} · goal -> income {:.0}, stability {:.1}, defense {:.1}, month {}",
                    system_name,
                    outcome.achieved_income,
                    outcome.achieved_stability,
                    outcome.achieved_defense,
                    outcome.months
                ),
                outcome.actions.clone(),
            )),
            SolveResult::Maximize(Some(outcome)) => Some((
                format!(
                    "{} · maximize {} -> income {:.0}, stability {:.1}, defense {:.1}, month {}",
                    system_name,
                    self.solve_params.maximize_metric.as_str(),
                    outcome.achieved_income,
                    outcome.achieved_stability,
                    outcome.achieved_defense,
                    outcome.months
                ),
                outcome.actions.clone(),
            )),
            _ => None,
        }
    }

    pub fn export_rank_csv(&mut self) {
        let rows = self.visible_rank_rows();
        let mut out = String::from("system,planets,score,score_per_planet,peak_income,seconds\n");
        for row in &rows {
            out.push_str(&format!(
                "{},{},{:.3},{:.3},{:.0},{:.2}\n",
                row.system,
                row.planet_count,
                row.solve.score,
                crate::rank::score_per_planet(row),
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
        if rows
            .iter()
            .any(|existing| existing.dir_name == row.dir_name)
        {
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

pub fn solve_cache_key(
    system_name: &str,
    params: &SolveParams,
    balance: &BalanceSignature,
) -> String {
    format!(
        "{system_name}|{:?}|{}|{}|{}|{:?}|{}|{}|{}|{}|{}|{:?}",
        params.mode,
        params.goal_income.to_bits(),
        params.goal_stability,
        params.goal_defense.to_bits(),
        params.maximize_metric,
        params.floor_income.to_bits(),
        params.floor_stability,
        params.floor_defense.to_bits(),
        params.horizon,
        params.time_limit,
        balance
    )
}

pub fn group_plan_actions(
    actions: &[Action],
    planet_names: &HashMap<u64, String>,
) -> Vec<PlanActionRow> {
    let mut month = 0;
    let mut rows = Vec::new();
    for (index, action) in actions.iter().enumerate() {
        match action {
            Action::Wait(months) => month += *months as i32,
            _ => rows.push(PlanActionRow {
                month,
                action_index: index,
                text: crate::solver::state::format_action(action, planet_names),
            }),
        }
    }
    rows
}

pub fn pareto_points(solve: &ParetoSolve) -> Vec<ParetoPoint> {
    solve
        .stability_frontier
        .iter()
        .chain(solve.defense_frontier.iter())
        .cloned()
        .collect()
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

fn rank_sort_name(mode: RankSortMode) -> &'static str {
    match mode {
        RankSortMode::ScorePerPlanet => "score/planet",
        RankSortMode::TotalScore => "score",
    }
}
