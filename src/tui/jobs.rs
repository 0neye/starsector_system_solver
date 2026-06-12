use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Instant;

use crate::extract::db::{Db, SaveRow, SystemDiscovery};
use crate::extract::gamedata::load_game_data;
use crate::extract::locate;
use crate::extract::mapping::map_save;
use crate::extract::model::{PlayerBalance, SaveInfo};
use crate::extract::save::{discover_saves, load_campaign_xml};
use crate::extract::scan::scan_save;
use crate::parser;
use crate::rank::{rank_systems, RankRow, RankScorer};
use crate::solve;
use crate::solver::{solve_pareto, Balance, Goal, Metric};
use crate::system::System;

use super::app::{PlanetDetail, SolveMode, SolveParams, SolveResult, SystemDetail};

pub enum Job {
    LoadSaves {
        db_path: PathBuf,
        starsector_dir: Option<PathBuf>,
    },
    Extract {
        db_path: PathBuf,
        starsector_dir: Option<PathBuf>,
        save_dir: PathBuf,
    },
    LoadSystems {
        db_path: PathBuf,
        save: String,
    },
    Rank {
        systems: HashMap<String, System>,
        names: Vec<String>,
        balance: Balance,
        horizon: i32,
        time_limit: u32,
        scorer: RankScorer,
        include_industry_upgrades: bool,
    },
    Solve {
        key: String,
        system_name: String,
        system: System,
        balance: Balance,
        params: SolveParams,
        include_industry_upgrades: bool,
    },
}

impl Job {
    /// Jobs whose worker polls `solver::cancel` and stops cooperatively.
    /// The others (IO-bound extraction/loading) can only be detached.
    pub fn supports_cancel(&self) -> bool {
        matches!(self, Job::Rank { .. } | Job::Solve { .. })
    }

    pub fn label(&self) -> &'static str {
        match self {
            Job::LoadSaves { .. } => "loading saves",
            Job::Extract { .. } => "extracting save",
            Job::LoadSystems { .. } => "loading systems",
            Job::Rank { .. } => "ranking systems",
            Job::Solve { .. } => "solving system",
        }
    }
}

#[derive(Debug, Clone)]
pub enum JobEvent {
    Progress(String),
    RankRow(RankRow),
    Done(JobOutput),
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum JobOutput {
    Saves {
        saves: Vec<SaveInfo>,
        extracted: Vec<SaveRow>,
        default_active: Option<SaveRow>,
    },
    Extracted(SaveRow),
    Systems {
        systems: HashMap<String, System>,
        discovery: Vec<SystemDiscovery>,
        details: HashMap<String, SystemDetail>,
        /// Balance-from-save seed (v1.5); None on pre-balance extractions.
        player_balance: Option<PlayerBalance>,
    },
    RankComplete(Vec<RankRow>),
    SolveComplete {
        key: String,
        result: SolveResult,
    },
}

pub struct JobRunner {
    receiver: Option<Receiver<JobEvent>>,
    label: Option<&'static str>,
    started_at: Option<Instant>,
    cancellable: bool,
    cancelling: bool,
}

impl JobRunner {
    pub fn new() -> Self {
        Self {
            receiver: None,
            label: None,
            started_at: None,
            cancellable: false,
            cancelling: false,
        }
    }

    pub fn start(&mut self, job: Job) {
        // A previous cancel must not leak into this job's solver loops.
        crate::solver::cancel::clear();
        let (tx, rx) = mpsc::channel();
        let label = job.label();
        self.receiver = Some(rx);
        self.label = Some(label);
        self.started_at = Some(Instant::now());
        self.cancellable = job.supports_cancel();
        self.cancelling = false;
        thread::spawn(move || run_job(job, tx));
    }

    /// Cooperatively cancel the running job if it supports it. Returns true
    /// when a cancel is (already) in flight; the job stays attached until the
    /// worker notices the flag and reports back.
    pub fn request_cancel(&mut self) -> bool {
        if !self.is_running() || !self.cancellable {
            return false;
        }
        crate::solver::cancel::request();
        self.cancelling = true;
        true
    }

    pub fn is_cancelling(&self) -> bool {
        self.cancelling
    }

    pub fn is_running(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn drain(&mut self) -> Vec<JobEvent> {
        let Some(rx) = &self.receiver else {
            return Vec::new();
        };
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    pub fn clear(&mut self) {
        self.receiver = None;
        self.label = None;
        self.started_at = None;
        self.cancellable = false;
        self.cancelling = false;
    }

    pub fn detach(&mut self) -> Option<&'static str> {
        let label = self.label;
        self.clear();
        label
    }

    pub fn started_at(&self) -> Option<Instant> {
        self.started_at
    }

    pub fn label(&self) -> Option<&'static str> {
        self.label
    }
}

impl Default for JobRunner {
    fn default() -> Self {
        Self::new()
    }
}

fn run_job(job: Job, tx: Sender<JobEvent>) {
    crate::cpu_affinity::prefer_performance_cores();

    let result = match job {
        Job::LoadSaves {
            db_path,
            starsector_dir,
        } => load_saves(db_path, starsector_dir, &tx),
        Job::Extract {
            db_path,
            starsector_dir,
            save_dir,
        } => extract_save(db_path, starsector_dir, save_dir, &tx),
        Job::LoadSystems { db_path, save } => load_systems(db_path, save, &tx),
        Job::Rank {
            systems,
            names,
            balance,
            horizon,
            time_limit,
            scorer,
            include_industry_upgrades,
        } => rank(
            systems,
            names,
            balance,
            horizon,
            time_limit,
            scorer,
            include_industry_upgrades,
            &tx,
        ),
        Job::Solve {
            key,
            system_name,
            system,
            balance,
            params,
            include_industry_upgrades,
        } => solve_job(
            key,
            system_name,
            system,
            balance,
            params,
            include_industry_upgrades,
            &tx,
        ),
    };

    match result {
        Ok(output) => {
            let _ = tx.send(JobEvent::Done(output));
        }
        Err(err) => {
            let _ = tx.send(JobEvent::Failed(err));
        }
    }
}

fn load_saves(
    db_path: PathBuf,
    starsector_dir: Option<PathBuf>,
    tx: &Sender<JobEvent>,
) -> Result<JobOutput, String> {
    let _ = tx.send(JobEvent::Progress("locating Starsector saves".to_string()));
    let saves = match locate::resolve_starsector_dir(starsector_dir.as_deref()) {
        Ok(dir) => {
            let saves_dir = locate::default_saves_dir(&dir);
            discover_saves(&saves_dir)
                .map_err(|err| format!("{err}; set STARSECTOR_DIR or Setup starsector_dir"))?
        }
        Err(err) => {
            if db_path.exists() {
                Vec::new()
            } else {
                return Err(format!("{err}; set STARSECTOR_DIR or Setup starsector_dir"));
            }
        }
    };
    let extracted = if db_path.exists() {
        let db = Db::open(&db_path).map_err(|err| format!("DB unreadable: {err}"))?;
        db.list_saves()
            .map_err(|err| format!("DB unreadable: {err}"))?
    } else {
        Vec::new()
    };
    let default_active = extracted.first().cloned();
    if saves.is_empty() && extracted.is_empty() {
        return Err("saves dir empty and no extracted saves in DB".to_string());
    }
    Ok(JobOutput::Saves {
        saves,
        extracted,
        default_active,
    })
}

fn extract_save(
    db_path: PathBuf,
    starsector_dir: Option<PathBuf>,
    save_dir: PathBuf,
    tx: &Sender<JobEvent>,
) -> Result<JobOutput, String> {
    let starsector_dir =
        locate::resolve_starsector_dir(starsector_dir.as_deref()).map_err(|err| err.to_string())?;
    let saves_dir = locate::default_saves_dir(&starsector_dir);
    let saves = discover_saves(&saves_dir).map_err(|err| err.to_string())?;
    let save = saves
        .iter()
        .find(|save| save.path == save_dir)
        .ok_or_else(|| "selected save disappeared from disk".to_string())?;
    let _ = tx.send(JobEvent::Progress(format!("reading {}", save.dir_name)));
    let campaign_xml = load_campaign_xml(save).map_err(|err| err.to_string())?;
    let raw = scan_save(&campaign_xml).map_err(|err| err.to_string())?;
    let _ = tx.send(JobEvent::Progress("loading game data".to_string()));
    let game_data = load_game_data(&starsector_dir).map_err(|err| err.to_string())?;
    let mapped = map_save(raw, &game_data);
    let mut db = Db::open(&db_path).map_err(|err| err.to_string())?;
    db.write_extraction(save, &game_data, &mapped)
        .map_err(|err| err.to_string())?;
    let extracted = db.list_saves().map_err(|err| err.to_string())?;
    let row = extracted
        .into_iter()
        .find(|row| row.dir_name == save.dir_name)
        .ok_or_else(|| "extracted save was not found in DB".to_string())?;
    Ok(JobOutput::Extracted(row))
}

fn load_systems(
    db_path: PathBuf,
    save: String,
    tx: &Sender<JobEvent>,
) -> Result<JobOutput, String> {
    let _ = tx.send(JobEvent::Progress("loading solver systems".to_string()));
    let systems =
        parser::load_game_data_from_db(&db_path, Some(&save)).map_err(|err| err.to_string())?;
    let db = Db::open(&db_path).map_err(|err| err.to_string())?;
    let discovery = db
        .system_discovery(Some(&save))
        .map_err(|err| err.to_string())?;
    let details = load_details(&db, &save, &systems)?;
    let player_balance = db.player_balance(Some(&save)).unwrap_or(None);
    Ok(JobOutput::Systems {
        systems,
        discovery,
        details,
        player_balance,
    })
}

fn load_details(
    db: &Db,
    save: &str,
    systems: &HashMap<String, System>,
) -> Result<HashMap<String, SystemDetail>, String> {
    let names: HashSet<String> = systems.keys().map(|name| name.to_lowercase()).collect();
    let rows = db
        .fetch_systems(Some(&save.to_lowercase()), Some(&names))
        .map_err(|err| err.to_string())?;
    let mut details = HashMap::new();
    for system_row in rows {
        let planets = db
            .fetch_planets(system_row.id, &system_row.name)
            .map_err(|err| err.to_string())?
            .into_iter()
            .map(|row| {
                let conditions = db.fetch_planet_conditions(row.id).unwrap_or_default();
                PlanetDetail { row, conditions }
            })
            .collect();
        let infrastructure = db
            .fetch_infrastructure(system_row.id, &system_row.name)
            .map_err(|err| err.to_string())?;
        details.insert(
            system_row.name.clone(),
            SystemDetail {
                name: system_row.name,
                planets,
                infrastructure,
            },
        );
    }
    Ok(details)
}

fn rank(
    systems: HashMap<String, System>,
    names: Vec<String>,
    balance: Balance,
    horizon: i32,
    time_limit: u32,
    scorer: RankScorer,
    include_industry_upgrades: bool,
    tx: &Sender<JobEvent>,
) -> Result<JobOutput, String> {
    let name_refs: Vec<&String> = names.iter().collect();
    let mut sent = 0usize;
    let total = name_refs.len();
    let rows = rank_systems(
        &systems,
        &balance,
        &name_refs,
        horizon,
        time_limit,
        scorer,
        include_industry_upgrades,
        &mut |row| {
            sent += 1;
            let _ = tx.send(JobEvent::RankRow(row.clone()));
            let _ = tx.send(JobEvent::Progress(format!("ranked {sent}/{total}")));
        },
    );
    Ok(JobOutput::RankComplete(rows))
}

fn solve_job(
    key: String,
    system_name: String,
    system: System,
    balance: Balance,
    params: SolveParams,
    include_industry_upgrades: bool,
    tx: &Sender<JobEvent>,
) -> Result<JobOutput, String> {
    let _ = tx.send(JobEvent::Progress(format!(
        "solving {system_name}; time budget is enforced, x cancels"
    )));
    let result = match params.mode {
        SolveMode::Pareto => SolveResult::Pareto(solve_pareto(
            &system,
            &balance,
            params.horizon,
            params.time_limit,
            include_industry_upgrades,
        )),
        SolveMode::Goal => {
            let goal = Goal::new(
                params.goal_income,
                Some(params.goal_defense),
                Some(params.goal_stability),
            );
            SolveResult::Goal(solve::solve_goal(
                &system,
                &balance,
                &goal,
                params.time_limit,
                include_industry_upgrades,
            ))
        }
        SolveMode::Maximize => {
            let floors = match params.maximize_metric {
                Metric::Income => Goal::new(
                    f64::NEG_INFINITY,
                    Some(params.floor_defense),
                    Some(params.floor_stability),
                ),
                Metric::Defense => {
                    Goal::new(params.floor_income, None, Some(params.floor_stability))
                }
                Metric::Stability => {
                    Goal::new(params.floor_income, Some(params.floor_defense), None)
                }
            };
            SolveResult::Maximize(solve::solve_maximize(
                &system,
                &balance,
                params.maximize_metric,
                &floors,
                params.horizon,
                params.time_limit,
                include_industry_upgrades,
            ))
        }
    };
    Ok(JobOutput::SolveComplete { key, result })
}
