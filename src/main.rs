mod constants;
mod cpu_affinity;
mod extract;
mod parser;
mod paths;
mod planet;
mod rank;
mod solve;
mod solver;
mod system;
mod tui;
mod utils;

use clap::{Parser, Subcommand, ValueEnum};
use constants::ColonyItem;
use extract::db::Db;
use extract::locate;
use planet::Planet;
use rank::{
    filter_system_names, peak_income, rank_systems, score_per_planet, sort_rows_best_first,
    RankScorer, RankSortMode,
};
use solver::Action;
use solver::{
    diagnose_maximize_gap, search_system_decomp, search_system_maximize,
    solve_pareto_with_settings, Balance, Goal, Metric, SolverSettings, State,
};
use std::collections::HashMap;
use std::error::Error;
use std::io;
use std::path::PathBuf;
use system::System;
use tui::config::{DiscoveryDefinition, TuiConfig};

#[derive(Parser)]
#[command(about = "Starsector colony system solver")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Star system to solve (name as extracted into the DB; see `extract search`)
    #[arg(long, default_value = "Mia's Star")]
    system: String,

    /// Extraction DB to load game data from (produced by `extract run`).
    /// Defaults to the per-user data dir, falling back to `./save_data.db`.
    #[arg(long, default_value_os_t = paths::default_db_path())]
    db: PathBuf,

    /// Extracted save to load (substring of save dir or character name).
    /// Defaults to the most recently extracted save.
    #[arg(long)]
    save: Option<String>,

    /// Minimum net income goal (credits/month). Defaults to 200000 in reach mode
    /// and to break-even (0) as a `--maximize` floor.
    #[arg(long)]
    income: Option<f64>,

    /// Minimum average stability goal
    #[arg(long)]
    stability: Option<i32>,

    /// Minimum average ground defense goal
    #[arg(long)]
    defense: Option<f64>,

    /// Score the system with Pareto frontiers and recommend a balanced plan.
    #[arg(long)]
    solve: bool,

    /// Starting credits
    #[arg(long, default_value_t = 5_000_000.0)]
    credits: f64,

    /// Starting story points
    #[arg(long, default_value_t = 5)]
    story_points: u32,

    /// Starting alpha cores
    #[arg(long, default_value_t = 1)]
    alpha_cores: u32,

    /// Disable story-point improvements and industry/structure alpha-core installs.
    /// They are included by default.
    #[arg(long = "no-industry-upgrades", action = clap::ArgAction::SetFalse, default_value_t = true)]
    include_industry_upgrades: bool,

    /// Modded behavior: allow multiple industries/structures to build at once
    /// on the same colony. By default, vanilla one-at-a-time colony build
    /// queueing is enforced.
    #[arg(long)]
    parallel_builds: bool,

    /// Colony item to start with (repeatable, e.g. --item "corrupted nanoforge")
    #[arg(long = "item")]
    items: Vec<ColonyItem>,

    /// Solver time budget in milliseconds
    #[arg(long, default_value_t = 25_000)]
    time_limit: u32,

    /// Rank systems by quick Pareto score (sparse floors, reduced search; see
    /// workspace/QUICK_RANKING_DESIGN.md). Ranks every system in the DB unless
    /// `--rank-system` filters are given. Ignores `--system`.
    #[arg(long)]
    rank: bool,

    /// Substring filter for `--rank` (repeatable; case-insensitive, any match)
    #[arg(long = "rank-system")]
    rank_systems: Vec<String>,

    /// Which `--rank` scorer to use: `quick` (default) = budgeted real search
    /// (`solve_pareto_quick`, Tier 1); `template` = instant template portfolio,
    /// in practice a Tier-0 lower bound (`solve_pareto_template`); `bound` =
    /// per-planet decomposed credit-relaxed near-certain upper bound, the
    /// Tier-0 "potential" ceiling (`solve_pareto_bound`). See
    /// workspace/QUICK_RANKING_DESIGN.md.
    #[arg(long = "rank-scorer", value_enum, default_value_t = RankScorer::Quick)]
    rank_scorer: RankScorer,

    /// Which systems `--rank` considers before name filters are applied.
    #[arg(long = "rank-scope", value_enum, default_value_t = RankScope::All)]
    rank_scope: RankScope,

    /// Discovery rule used by `--rank-scope discovered`.
    #[arg(
        long = "discovery-definition",
        value_enum,
        default_value_t = DiscoveryDefinition::AtLeastOneSurveyed
    )]
    discovery_definition: DiscoveryDefinition,

    /// Include core-world systems when using `--rank-scope discovered`.
    #[arg(long)]
    include_core_worlds: bool,

    /// Sort `--rank` output by score per planet or total score.
    #[arg(long = "rank-sort", value_enum, default_value_t = RankSortMode::ScorePerPlanet)]
    rank_sort: RankSortMode,

    /// Emit `--rank` results as CSV (system,score,peak_income,seconds) instead
    /// of the human-readable table — used by the rank-validation harness.
    #[arg(long)]
    rank_csv: bool,

    /// Maximize one metric instead of reaching a fixed goal. The maximized
    /// metric's own threshold is ignored; the other two `--income/--stability/
    /// --defense` values act as floors that must be held.
    #[arg(long, value_parser = ["income", "defense", "stability"])]
    maximize: Option<String>,

    /// Game-month horizon for `--maximize`: push the metric as high as possible
    /// within this many months.
    #[arg(long, default_value_t = 120)]
    horizon: i32,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Save-game extraction tools (parse saves into the DB, search, export)
    #[command(subcommand)]
    Extract(extract::cli::ExtractCommand),
    /// Locate the Starsector install and its saves directory.
    Locate {
        /// Starsector install directory. When omitted, auto-detected from the
        /// STARSECTOR_DIR environment variable or common install locations.
        #[arg(long)]
        starsector_dir: Option<PathBuf>,
    },
    /// Save the Starsector install path and build the first extraction DB.
    Init {
        /// Starsector install directory. When omitted, auto-detected from the
        /// STARSECTOR_DIR environment variable or common install locations.
        #[arg(long)]
        starsector_dir: Option<PathBuf>,
        /// Save substring to extract. When omitted, `--latest` is used by
        /// default so a no-argument installer run can initialize the DB.
        #[arg(long)]
        save: Option<String>,
        /// Extract the newest save.
        #[arg(long)]
        latest: bool,
        /// Output DB. Defaults to the per-user data dir.
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Inspect extracted system planet, resource, and infrastructure data.
    Inspect {
        /// Extraction DB to inspect. Defaults to the per-user data dir.
        #[arg(long, default_value_os_t = paths::default_db_path())]
        db: PathBuf,
        /// Extracted save to inspect. Defaults to the most recently extracted save.
        #[arg(long)]
        save: Option<String>,
        /// Show every system in the save.
        #[arg(long)]
        all: bool,
        /// System name substring(s) to inspect.
        #[arg(value_name = "SYSTEM")]
        systems: Vec<String>,
    },
    /// Open the interactive terminal UI.
    Tui {
        /// Starsector install directory. Overrides workspace/solver_tui.toml for this run.
        #[arg(long)]
        starsector_dir: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum RankScope {
    All,
    Discovered,
}

/// Load solver game data from the extraction DB. Env-var modes use
/// `SYSTEM_SOLVER_DB` / `SYSTEM_SOLVER_SAVE` since they bypass CLI parsing.
fn load_systems_from_env() -> Result<HashMap<String, System>, Box<dyn Error>> {
    // `paths::default_db_path` already honors `SYSTEM_SOLVER_DB`, then the
    // per-user data dir, then `./save_data.db`.
    let db = paths::default_db_path();
    let save = std::env::var("SYSTEM_SOLVER_SAVE").ok();
    Ok(parser::load_game_data_from_db(&db, save.as_deref())?)
}

fn env_solver_settings(include_industry_upgrades: bool) -> SolverSettings {
    SolverSettings {
        include_industry_upgrades,
        allow_parallel_builds: std::env::var_os("SYSTEM_SOLVER_PARALLEL_BUILDS")
            .is_some_and(|value| !value.is_empty() && value != "0"),
    }
}

/// Run the maximize-mode solver and report the best plan plus the metric values
/// it actually achieves (by replaying the solution onto a fresh copy of `state`).
fn test_maximize(
    mut state: State,
    metric: Metric,
    floors: &Goal,
    horizon: i32,
    time_limit: u32,
    settings: SolverSettings,
) {
    println!(
        "\nStarting maximize solver ({}, horizon {} months)...",
        metric.as_str(),
        horizon
    );

    let replay_base = state.clone();
    let results = solver::search_system_maximize_with_settings(
        &mut state, metric, floors, horizon, time_limit, settings,
    );

    if results.is_empty() {
        println!("No plan satisfies the floors within the horizon.");
        return;
    }

    for (i, result) in results.iter().enumerate() {
        let mut replay = replay_base.clone();
        for a in result.solution.iter().flatten() {
            replay.apply_action_raw(a, false);
        }
        println!(
            "Result {}: best {} reached at month {}",
            i + 1,
            metric.as_str(),
            result.cost,
        );
        println!(
            "    income={:.0}  stability={:.1}  defense={:.1}",
            replay.balance().net_income(),
            replay.system().avg_stability(),
            replay.system().avg_ground_defense(),
        );
        println!("{:#?}", result);
    }
}

fn test_solver(mut state: State, goal: &Goal, time_limit: u32, settings: SolverSettings) {
    println!("\nStarting solver test...");
    println!("Initial state score: {}", state.score() as i32);

    // Default solver: the joint two-level decomposition (shared timeline).
    let results =
        solver::search_system_decomp_with_settings(&mut state, goal, time_limit, settings);

    if results.is_empty() {
        println!("No solution found within time limit.");
    } else {
        // Print all results found
        for (i, result) in results.iter().enumerate() {
            println!("Result {}:\n{:#?}", i + 1, result);
        }
    }
}

fn planet_name_map(system: &System) -> HashMap<u64, String> {
    system
        .planets()
        .iter()
        .map(|(hash, planet)| (*hash, planet.name().to_string()))
        .collect()
}

fn print_action_log(actions: &[Action], planet_names: &HashMap<u64, String>) {
    for (i, action) in actions.iter().enumerate() {
        println!(
            "    {:>2}. {}",
            i + 1,
            solver::state::format_action(action, planet_names)
        );
    }
}

fn run_solve(
    system_name: &str,
    system: &System,
    balance: &Balance,
    horizon: i32,
    time_limit: u32,
    settings: SolverSettings,
) {
    println!("Pareto solve for {system_name} (horizon {horizon} months)");
    println!(
        "Starting balance: credits={:.0}, story_points={}, alpha_cores={}",
        balance.credits(),
        balance.story_points(),
        balance.alpha_cores(),
    );

    let solution = solve_pareto_with_settings(system, balance, horizon, time_limit, settings);

    println!("\nSystem score: {:.1}", solution.score);
    println!(
        "  stability normalized AUC: {:.0} credits/month",
        solution.stability_auc
    );
    println!(
        "  defense normalized AUC:   {:.0} credits/month",
        solution.defense_auc
    );

    println!("\nStability frontier:");
    for point in &solution.stability_frontier {
        println!(
            "  floor {:>4.0} -> income={:>9.0}, stability={:>4.1}, defense={:>7.1}, month={}",
            point.floor, point.income, point.stability, point.defense, point.months,
        );
    }

    println!("\nDefense frontier:");
    for point in &solution.defense_frontier {
        println!(
            "  floor {:>4.0} -> income={:>9.0}, stability={:>4.1}, defense={:>7.1}, month={}",
            point.floor, point.income, point.stability, point.defense, point.months,
        );
    }

    if let Some(point) = solution.recommendation {
        println!(
            "\nRecommended tradeoff: {} floor {:.0} -> income={:.0}, stability={:.1}, defense={:.1} at month {}",
            point.kind.as_str(),
            point.floor,
            point.income,
            point.stability,
            point.defense,
            point.months,
        );
        println!("Action sequence:");
        print_action_log(&point.actions, &planet_name_map(system));
    } else {
        println!("\nNo feasible Pareto point found within the time budget.");
    }
}

/// `--rank`: score every selected system with a quick Pareto sweep and print
/// them best-first. `scorer` selects the strategy (see [`RankScorer`]): `Quick`
/// is the Tier-1 budgeted search; `Template` and `Bound` are the Tier-0 instant
/// scorers (in practice a lower and an upper bound on the score). All are
/// deterministic and meant for *ordering* systems, not as final numbers —
/// `--solve` on the chosen system gives the real frontier. See
/// workspace/QUICK_RANKING_DESIGN.md.
#[allow(clippy::too_many_arguments)]
fn run_rank(
    systems: &HashMap<String, System>,
    balance: &Balance,
    db_path: &PathBuf,
    save: Option<&str>,
    filters: &[String],
    scope: RankScope,
    discovery_definition: DiscoveryDefinition,
    include_core_worlds: bool,
    horizon: i32,
    time_limit: u32,
    csv: bool,
    scorer: RankScorer,
    sort_mode: RankSortMode,
    settings: SolverSettings,
) {
    let names = rank_names(
        systems,
        db_path,
        save,
        filters,
        scope,
        discovery_definition,
        include_core_worlds,
    )
    .unwrap_or_else(|err| {
        eprintln!("{err}");
        std::process::exit(1);
    });

    if names.is_empty() {
        eprintln!("error: no system matches the selected rank scope/filter(s)");
        std::process::exit(1);
    }

    eprintln!(
        "Ranking {} systems ({scorer:?} scorer, {sort_mode:?} sort, horizon {horizon} months)...",
        names.len()
    );

    let mut ranked = rank_systems(
        systems,
        balance,
        &names,
        horizon,
        time_limit,
        scorer,
        settings,
        &mut |row| {
            eprintln!(
                "  [{}] score {:.1} ({:.1}/planet) in {:.1}s",
                row.system,
                row.solve.score,
                score_per_planet(row),
                row.seconds
            );
        },
    );
    sort_rows_best_first(&mut ranked, sort_mode);

    if csv {
        println!("system,score,peak_income,seconds");
        for row in &ranked {
            println!(
                "{},{:.3},{:.0},{:.2}",
                row.system,
                row.solve.score,
                peak_income(&row.solve),
                row.seconds
            );
        }
        return;
    }

    println!(
        "\n#   {:<24} {:>7}  {:>8}  {:>10}  {:>12}  {:>7}",
        "system", "planets", "score", "score/pl", "peak income", "time"
    );
    for (i, row) in ranked.iter().enumerate() {
        println!(
            "{:<3} {:<24} {:>7}  {:>8.1}  {:>10.1}  {:>12.0}  {:>6.1}s",
            i + 1,
            row.system,
            row.planet_count,
            row.solve.score,
            score_per_planet(row),
            peak_income(&row.solve),
            row.seconds,
        );
    }
    println!(
        "\nQuick scores are deterministic approximations meant for ordering; run\n\
         --solve --system <NAME> on the system you pick for the real frontier."
    );
}

/// Build a representative starting balance (generous credits plus a couple of
/// colony items) matching the default `main` setup. Used by the verify harness.
fn ab_balance() -> Balance {
    let mut balance = Balance::new(5_000_000.0, 5, 1);
    balance.add_colony_item(ColonyItem::CorruptedNanoforge);
    balance.add_colony_item(ColonyItem::SoilNanites);
    balance
}

/// Independently verify the joint decomp solver: solve the whole system, then
/// replay the returned log on a *fresh* copy of the full system and report the
/// recomputed net income, stability, unfinished facilities, and — crucially —
/// whether any one-off resource (alpha cores / story points) went negative,
/// which would mean a shared resource was double-spent. Trusts nothing about the
/// simulator's incremental bookkeeping.
fn verify_decomp(systems: &HashMap<String, System>, settings: SolverSettings) {
    let goal = Goal::new(40_000.0, None, Some(6));
    let mut names: Vec<&String> = systems.keys().collect();
    names.sort();

    println!("\n--------- joint decomp verification (goal income>=40k, stab>=6) ---------");
    for name in names {
        let mut state = State::new(ab_balance(), systems[name].clone());
        let planet_count = state.system().planets().len();

        let Some(result) =
            solver::decomp::decomp_search_with_settings(&mut state, &goal, 50000, settings)
        else {
            println!("### {name}: no solution");
            continue;
        };
        let log = result.solution.unwrap();

        // Replay from scratch on the full system.
        let mut replay = State::new(ab_balance(), systems[name].clone());
        for a in &log {
            replay.apply_action_raw(a, false);
        }

        let unfinished = replay
            .system()
            .planets()
            .values()
            .flat_map(|p| p.facilities().iter())
            .filter(|f| f.remaining_build_days() > 0)
            .count();
        let colonized = replay
            .system()
            .planets()
            .values()
            .filter(|p| p.has_colony())
            .count();

        println!(
            "### {name} ({planet_count} planets): cost={}mo  colonized={colonized}",
            result.cost
        );
        println!(
            "    replay net_income={:.0}  stab={:.1}  unfinished_facilities={}  satisfied={}",
            replay.balance().net_income(),
            replay.system().avg_stability(),
            unfinished,
            goal.is_satisfied_quiet(&replay),
        );
        println!(
            "    remaining resources: credits={:.0}  story_points={}  alpha_cores={}  (none should be impossible)",
            replay.balance().credits(),
            replay.balance().story_points(),
            replay.balance().alpha_cores(),
        );
    }
    println!("------------------------------------------------------------------------\n");
}

/// Measure, for every loaded system, how much income the greedy maximize solver
/// leaves on the table relative to a **budget-relaxed upper bound** (the same
/// search run from an unlimited starting balance — see [`solver::bound`]). For
/// each stability and defense floor it prints a CSV row of greedy vs bound income
/// and the gap, then a summary of the worst gaps. This is the measurement that
/// decides whether an exact/branch-and-bound solver is worth building: small gaps
/// everywhere mean the greedy is already near-optimal on the budget axis.
///
/// Both runs use the same warm-start chaining and concurrent stability/defense
/// chains as `solve_pareto`, and each bound solve is additionally cross-seeded
/// with the same floor's greedy plan (feasible and no worse under relaxed
/// credits), so `bound >= greedy` holds by construction and a negative gap no
/// longer reflects bound-search suboptimality.
/// Triggered by `SYSTEM_SOLVER_BOUND=1`; horizon/budget via
/// `SYSTEM_SOLVER_BOUND_HORIZON` / `SYSTEM_SOLVER_BOUND_MS`;
/// `SYSTEM_SOLVER_BOUND_SYSTEM=<substring>` limits the sweep to one system;
/// `SYSTEM_SOLVER_NO_UPGRADES=1` disables improvements/alpha-core installs
/// (matches the CLI's `--no-industry-upgrades`).
fn run_bound(systems: &HashMap<String, System>, horizon: i32, time_limit: u32) {
    use solver::decomp::{SearchProfile, SystemPlan};
    use solver::pareto::{
        measure_point_seeded, FrontierKind, ParetoPoint, DEFENSE_FLOORS, STABILITY_FLOORS,
    };
    use solver::{credits_relaxed, BoundRow};

    // The same scenario the Pareto sweep and A/B harness use, so numbers line up.
    let base = ab_balance();
    let relaxed = credits_relaxed(&base);

    // One floor's (greedy, bound) pair. The greedy run warm-starts from its
    // chain's previous plan (same chaining as `solve_pareto`); the bound run
    // additionally seeds the greedy plan it is compared against — feasible and
    // no worse under relaxed credits — so a negative gap can no longer mean
    // "the relaxed search failed to find what the greedy found".
    #[allow(clippy::too_many_arguments)] // mirrors measure_point_seeded
    fn measure_pair(
        system: &System,
        base: &Balance,
        relaxed: &Balance,
        kind: FrontierKind,
        floor: f64,
        floors: &Goal,
        horizon: i32,
        time_limit: u32,
        settings: SolverSettings,
        warm_greedy: &mut Option<SystemPlan>,
        warm_bound: &mut Option<SystemPlan>,
    ) -> (Option<ParetoPoint>, Option<ParetoPoint>) {
        let greedy = measure_point_seeded(
            system,
            base,
            kind,
            floor,
            floors,
            horizon,
            time_limit,
            settings,
            warm_greedy,
            &[],
            SearchProfile::FULL,
        );
        let cross: Vec<SystemPlan> = warm_greedy.iter().cloned().collect();
        let bound = measure_point_seeded(
            system,
            relaxed,
            kind,
            floor,
            floors,
            horizon,
            time_limit,
            settings,
            warm_bound,
            &cross,
            SearchProfile::FULL,
        );
        (greedy, bound)
    }

    let settings = env_solver_settings(std::env::var_os("SYSTEM_SOLVER_NO_UPGRADES").is_none());

    let mut names: Vec<&String> = systems.keys().collect();
    names.sort();
    // Optional substring filter so a single system can be measured.
    if let Ok(filter) = std::env::var("SYSTEM_SOLVER_BOUND_SYSTEM") {
        if !filter.is_empty() {
            let needle = filter.to_lowercase();
            names.retain(|n| n.to_lowercase().contains(&needle));
        }
    }

    let mut rows: Vec<BoundRow> = Vec::new();

    eprintln!(
        "Computing greedy-vs-bound gaps (horizon {horizon} months, {time_limit}ms/point, {} systems)...",
        names.len()
    );
    println!("system,kind,floor,greedy_income,bound_income,gap,gap_pct,greedy_months,bound_months");

    for name in &names {
        let system = &systems[*name];
        let system_start = std::time::Instant::now();

        // First stability point alone, so its plans can seed both chains; then
        // the remaining stability floors and the defense floors run as two
        // concurrent warm-start chains (mirrors `solve_pareto`; `System` holds
        // `RefCell` caches so each chain gets its own clone).
        let mut stab_warm_greedy = None;
        let mut stab_warm_bound = None;
        let first_stab = STABILITY_FLOORS[0];
        let first_floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(first_stab));
        let first_pair = measure_pair(
            system,
            &base,
            &relaxed,
            FrontierKind::Stability,
            first_stab as f64,
            &first_floors,
            horizon,
            time_limit,
            settings,
            &mut stab_warm_greedy,
            &mut stab_warm_bound,
        );
        let mut def_warm_greedy = stab_warm_greedy.clone();
        let mut def_warm_bound = stab_warm_bound.clone();

        let stability_system = system.clone();
        let stability_base = base.clone();
        let stability_relaxed = relaxed.clone();
        let defense_system = system.clone();
        let defense_base = base.clone();
        let defense_relaxed = relaxed.clone();
        let (stab_tail, def_pairs) = std::thread::scope(|scope| {
            let stability_handle = scope.spawn(move || {
                cpu_affinity::prefer_performance_cores();

                let mut points = Vec::new();
                for stab in STABILITY_FLOORS.iter().copied().skip(1) {
                    let floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(stab));
                    points.push((
                        stab as f64,
                        measure_pair(
                            &stability_system,
                            &stability_base,
                            &stability_relaxed,
                            FrontierKind::Stability,
                            stab as f64,
                            &floors,
                            horizon,
                            time_limit,
                            settings,
                            &mut stab_warm_greedy,
                            &mut stab_warm_bound,
                        ),
                    ));
                }
                points
            });
            let defense_handle = scope.spawn(move || {
                cpu_affinity::prefer_performance_cores();

                let mut points = Vec::new();
                for def_floor in DEFENSE_FLOORS {
                    let floors = Goal::new(f64::NEG_INFINITY, Some(def_floor), Some(0));
                    points.push((
                        def_floor,
                        measure_pair(
                            &defense_system,
                            &defense_base,
                            &defense_relaxed,
                            FrontierKind::Defense,
                            def_floor,
                            &floors,
                            horizon,
                            time_limit,
                            settings,
                            &mut def_warm_greedy,
                            &mut def_warm_bound,
                        ),
                    ));
                }
                points
            });
            (
                stability_handle.join().expect("stability chain panicked"),
                defense_handle.join().expect("defense chain panicked"),
            )
        });

        let mut push =
            |kind: FrontierKind, floor: f64, pair: (Option<ParetoPoint>, Option<ParetoPoint>)| {
                let (greedy, bound) = pair;
                let row = BoundRow {
                    system: (*name).clone(),
                    kind: kind.as_str(),
                    floor,
                    greedy_income: greedy.as_ref().map(|p| p.income),
                    bound_income: bound.as_ref().map(|p| p.income),
                    greedy_months: greedy.as_ref().map(|p| p.months),
                    bound_months: bound.as_ref().map(|p| p.months),
                };
                let fmt_f = |v: Option<f64>| v.map_or("--".to_string(), |x| format!("{x:.0}"));
                let fmt_i = |v: Option<i32>| v.map_or("--".to_string(), |x| x.to_string());
                println!(
                    "{},{},{:.0},{},{},{},{},{},{}",
                    row.system,
                    row.kind,
                    row.floor,
                    fmt_f(row.greedy_income),
                    fmt_f(row.bound_income),
                    fmt_f(row.gap()),
                    row.gap_pct()
                        .map_or("--".to_string(), |p| format!("{p:.1}")),
                    fmt_i(row.greedy_months),
                    fmt_i(row.bound_months),
                );
                rows.push(row);
            };

        push(FrontierKind::Stability, first_stab as f64, first_pair);
        for (floor, pair) in stab_tail {
            push(FrontierKind::Stability, floor, pair);
        }
        for (floor, pair) in def_pairs {
            push(FrontierKind::Defense, floor, pair);
        }
        eprintln!(
            "[{name}] done in {:.1}s",
            system_start.elapsed().as_secs_f64()
        );
    }

    // ----- Summary -----
    let mut gaps: Vec<(&BoundRow, f64)> = rows
        .iter()
        .filter_map(|r| r.gap_pct().map(|p| (r, p)))
        .collect();
    gaps.sort_by(|a, b| b.1.total_cmp(&a.1));

    let comparable = gaps.len();
    let greedy_infeasible = rows
        .iter()
        .filter(|r| r.greedy_income.is_none() && r.bound_income.is_some())
        .count();

    eprintln!("\n================ greedy-vs-bound summary ================");
    if comparable == 0 {
        eprintln!("No comparable (both-feasible, positive-greedy) points.");
    } else {
        let mean = gaps.iter().map(|(_, p)| *p).sum::<f64>() / comparable as f64;
        let median = gaps[comparable / 2].1;
        let over_5 = gaps.iter().filter(|(_, p)| *p > 5.0).count();
        let over_15 = gaps.iter().filter(|(_, p)| *p > 15.0).count();
        // Cross-seeding makes bound >= greedy hold by construction, so the
        // most-negative gap should sit at ~0; anything clearly below that means
        // the relaxed simulation of the greedy plan came out *worse* than the
        // real-credit one, which would be a solver bug worth investigating.
        let noise_floor = gaps.iter().map(|(_, p)| *p).fold(f64::INFINITY, f64::min);
        eprintln!("comparable points : {comparable}");
        eprintln!("mean gap          : {mean:.1}%");
        eprintln!("median gap        : {median:.1}%");
        eprintln!("points >5% gap    : {over_5}");
        eprintln!("points >15% gap   : {over_15}");
        eprintln!("noise floor (min) : {noise_floor:.1}%  (cross-seeded: should be ~0; clearly negative would indicate a solver bug)");
        eprintln!("\nworst 10 gaps (bound is an *upper* estimate of headroom):");
        for (r, p) in gaps.iter().take(10) {
            eprintln!(
                "  {:>5.1}%  {:<16} {}>={:<5.0}  greedy={:>9.0}  bound={:>9.0}",
                p,
                r.system,
                r.kind,
                r.floor,
                r.greedy_income.unwrap_or(f64::NAN),
                r.bound_income.unwrap_or(f64::NAN),
            );
        }
    }

    if greedy_infeasible > 0 {
        eprintln!(
            "\nnote: {greedy_infeasible} point(s) where the budget-relaxed search found a \
             floor-feasible plan but the real-budget greedy did not (budget was the blocker)."
        );
    }
    eprintln!("========================================================\n");
}

fn rank_names<'a>(
    systems: &'a HashMap<String, System>,
    db_path: &PathBuf,
    save: Option<&str>,
    filters: &[String],
    scope: RankScope,
    discovery_definition: DiscoveryDefinition,
    include_core_worlds: bool,
) -> Result<Vec<&'a String>, String> {
    let mut names = filter_system_names(systems, filters)
        .map_err(|_| "error: no system matches the --rank-system filter(s)".to_string())?;

    if scope == RankScope::All {
        return Ok(names);
    }

    let db = Db::open(db_path).map_err(|err| format!("error: DB unreadable: {err}"))?;
    let discovery = db
        .system_discovery(save)
        .map_err(|err| format!("error: discovery data unavailable: {err}"))?;
    let allowed: std::collections::HashSet<String> = discovery
        .into_iter()
        .filter(|row| include_core_worlds || !row.is_core)
        .filter(|row| match discovery_definition {
            DiscoveryDefinition::AtLeastOneSurveyed => row.surveyed_any >= 1,
            DiscoveryDefinition::FullySurveyed => {
                row.planet_count > 0 && row.surveyed_full == row.planet_count
            }
        })
        .map(|row| row.system_name)
        .collect();
    names.retain(|name| allowed.contains(name.as_str()));
    Ok(names)
}

fn inspect_systems(
    db_path: &PathBuf,
    save: Option<&str>,
    all: bool,
    filters: &[String],
) -> Result<(), Box<dyn Error>> {
    if !all && filters.is_empty() {
        return Err("inspect needs at least one SYSTEM substring or --all".into());
    }

    let db = Db::open(db_path)?;
    let save_filter = save.map(|value| value.to_lowercase());
    let systems = db.fetch_systems(save_filter.as_deref(), None)?;
    let needles: Vec<String> = filters.iter().map(|value| value.to_lowercase()).collect();
    let matched: Vec<_> = systems
        .into_iter()
        .filter(|system| {
            all || needles.iter().any(|needle| {
                system.name.to_lowercase().contains(needle)
                    || system.display_name.to_lowercase().contains(needle)
            })
        })
        .collect();

    if matched.is_empty() {
        return Err("no system matched the inspect filter(s)".into());
    }

    for (index, system) in matched.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!("System: {}", system.name);
        if system.display_name != system.name {
            println!("  display: {}", system.display_name);
        }
        println!("  save: {}", system.save_dir_name);
        println!("  internal id: {}", system.internal_id);
        println!(
            "  location: x={}, y={}, distance={}",
            fmt_optional_f64(system.x_ly, " ly"),
            fmt_optional_f64(system.y_ly, " ly"),
            fmt_optional_f64(system.dist_from_com_ly, " ly"),
        );
        println!(
            "  stable points: {} | gate: {} | remnants: {}{}",
            system.stable_points,
            yes_no(system.has_gate),
            yes_no(system.has_remnants),
            if system.remnant_damaged {
                " (damaged)"
            } else {
                ""
            }
        );
        println!(
            "  stars: {}",
            if system.star_types.is_empty() {
                "-".to_string()
            } else {
                system.star_types.join(", ")
            }
        );

        let infrastructure = db.fetch_infrastructure(system.id, &system.name)?;
        if infrastructure.is_empty() {
            println!("  infrastructure: -");
        } else {
            println!("  infrastructure:");
            for row in infrastructure {
                println!(
                    "    - {}{}{}",
                    row.infrastructure_type,
                    if row.is_domain { " [domain]" } else { "" },
                    if row.is_damaged { " [damaged]" } else { "" },
                );
            }
        }

        let planets = db.fetch_planets(system.id, &system.name)?;
        println!("  planets: {}", planets.len());
        for planet in planets {
            let conditions = db.fetch_planet_conditions(planet.id).unwrap_or_default();
            println!(
                "    - {}: {}{} | hazard {:.0}% | survey {} | owner {}",
                planet.name,
                planet.planet_type,
                if planet.is_moon { " moon" } else { "" },
                planet.hazard_percent,
                planet.survey_level.as_deref().unwrap_or("-"),
                planet.owner_faction.as_deref().unwrap_or("-"),
            );
            println!(
                "      resources: farmland {}, ores {}, rare ores {}, volatiles {}, organics {}, ruins {}",
                fmt_optional_f64(planet.farmland, ""),
                fmt_optional_f64(planet.ores, ""),
                fmt_optional_f64(planet.rare_ores, ""),
                fmt_optional_f64(planet.volatiles, ""),
                fmt_optional_f64(planet.organics, ""),
                fmt_optional_f64(planet.ruins, ""),
            );
            println!(
                "      flags: accessibility {}, radius {:.0}, no_atmosphere {}, very_hot {}, gas_giant {}, habitable {}, extreme_activity {}, water {}, hazard_incomplete {}",
                fmt_optional_f64(planet.accessibility_percent, "%"),
                planet.radius,
                yes_no(planet.no_atmosphere),
                yes_no(planet.very_hot),
                yes_no(planet.gas_giant),
                yes_no(planet.habitable),
                yes_no(planet.extreme_activity),
                yes_no(planet.water),
                yes_no(planet.hazard_incomplete),
            );
            println!(
                "      conditions: {}",
                if conditions.is_empty() {
                    "-".to_string()
                } else {
                    conditions.join(", ")
                }
            );
        }
    }

    Ok(())
}

fn fmt_optional_f64(value: Option<f64>, suffix: &str) -> String {
    value
        .map(|value| format!("{value:.1}{suffix}"))
        .unwrap_or_else(|| "-".to_string())
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

fn run_locate(starsector_dir: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let starsector_dir = locate::resolve_starsector_dir(starsector_dir.as_deref())?;
    let saves_dir = locate::default_saves_dir(&starsector_dir);
    println!("Starsector: {}", starsector_dir.display());
    println!("Saves: {}", saves_dir.display());
    Ok(())
}

fn run_init(
    starsector_dir: Option<PathBuf>,
    save: Option<String>,
    latest: bool,
    db: Option<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    let starsector_dir = locate::resolve_starsector_dir(starsector_dir.as_deref())?;
    let db = db.unwrap_or_else(paths::default_db_path);
    let config_path = paths::config_path();
    let effective_latest = latest || save.is_none();

    let (mut config, warning) = TuiConfig::load(&config_path);
    if let Some(warning) = warning {
        eprintln!("warning: {warning}");
    }
    config.starsector_dir = Some(starsector_dir.clone());
    config.db_path = db.clone();
    config.save(&config_path).map_err(io::Error::other)?;

    extract::cli::run(extract::cli::ExtractCommand::Run {
        saves_dir: None,
        save,
        latest: effective_latest,
        starsector_dir: Some(starsector_dir.clone()),
        db: db.clone(),
        system: vec![],
    })?;

    println!("Initialized Starsector System Ranker.");
    println!("Starsector: {}", starsector_dir.display());
    println!("DB: {}", db.display());
    println!("Config: {}", config_path.display());
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    cpu_affinity::prefer_performance_cores();

    // Special env-var modes bypass normal CLI parsing.
    if std::env::var_os("SYSTEM_SOLVER_BOUND").is_some() {
        eprintln!("Loading game data...");
        let systems = load_systems_from_env()?;
        let horizon: i32 = std::env::var("SYSTEM_SOLVER_BOUND_HORIZON")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(120);
        let time_limit: u32 = std::env::var("SYSTEM_SOLVER_BOUND_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2_000);
        run_bound(&systems, horizon, time_limit);
        return Ok(());
    }

    if std::env::var_os("SYSTEM_SOLVER_VERIFY").is_some() {
        println!("Loading game data...");
        let systems = load_systems_from_env()?;
        verify_decomp(&systems, env_solver_settings(true));
        return Ok(());
    }

    // Maximize local-minimum diagnostic: reproduce the Mia Bravos
    // `--maximize income --stability 6` gap and report which move type bridges it.
    // SYSTEM_SOLVER_DIAG=<system> overrides the system (default "Mia Bravos").
    if let Some(diag) = std::env::var_os("SYSTEM_SOLVER_DIAG") {
        println!("Loading game data...");
        let systems = load_systems_from_env()?;
        let sys_name = diag
            .to_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("Mia Bravos");
        let system = systems
            .get(sys_name)
            .unwrap_or_else(|| panic!("diagnostic system {sys_name:?} not found"))
            .clone();
        // Match the CLI defaults so the diagnostic reproduces the real repro.
        let state = State::new(Balance::new(5_000_000.0, 5, 1), system);
        let floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(6));
        println!(
            "{}",
            diagnose_maximize_gap(&state, Metric::Income, &floors, 120, true)
        );
        return Ok(());
    }

    // Pareto-frontier sweep used by plot_pareto_frontiers.py. For each system,
    // maximize income while sweeping the stability floor (5..=10)
    // and the ground-defense floor, printing CSV rows of the achieved
    // (income, stability, defense). Triggered by SYSTEM_SOLVER_PARETO=1.
    //
    // Configurable via env vars:
    //   SYSTEM_SOLVER_PARETO_HORIZON   — game-month horizon (default 120)
    //   SYSTEM_SOLVER_PARETO_MS        — solver time budget ms (default 5000)
    //   SYSTEM_SOLVER_PARETO_CREDITS   — starting credits (default 5_000_000)
    //   SYSTEM_SOLVER_PARETO_SP        — starting story points (default 5)
    //   SYSTEM_SOLVER_PARETO_ALPHA     — starting alpha cores (default 1)
    //   SYSTEM_SOLVER_PARETO_ALL_ITEMS — add N of every colony item (default 0)
    if std::env::var_os("SYSTEM_SOLVER_PARETO").is_some() {
        let systems = load_systems_from_env()?;
        let horizon: i32 = std::env::var("SYSTEM_SOLVER_PARETO_HORIZON")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(120);
        let time_limit: u32 = std::env::var("SYSTEM_SOLVER_PARETO_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5_000);
        let credits: f64 = std::env::var("SYSTEM_SOLVER_PARETO_CREDITS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5_000_000.0);
        let sp: u32 = std::env::var("SYSTEM_SOLVER_PARETO_SP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5);
        let alpha: u32 = std::env::var("SYSTEM_SOLVER_PARETO_ALPHA")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1);
        let all_items_count: u32 = std::env::var("SYSTEM_SOLVER_PARETO_ALL_ITEMS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let mut base_balance = Balance::new(credits, sp, alpha);
        if all_items_count > 0 {
            for item in ColonyItem::all() {
                for _ in 0..all_items_count {
                    base_balance.add_colony_item(item);
                }
            }
        }

        let mut names: Vec<&String> = systems.keys().collect();
        names.sort();
        // Optional substring filter so a single system can be benchmarked.
        if let Ok(filter) = std::env::var("SYSTEM_SOLVER_PARETO_SYSTEM") {
            if !filter.is_empty() {
                let needle = filter.to_lowercase();
                names.retain(|n| n.to_lowercase().contains(&needle));
            }
        }
        let show_stats = std::env::var_os("SYSTEM_SOLVER_STATS").is_some();
        // SYSTEM_SOLVER_NO_UPGRADES=1 disables improvements/alpha-core installs
        // (matches the CLI's `--no-industry-upgrades`).
        let settings = env_solver_settings(std::env::var_os("SYSTEM_SOLVER_NO_UPGRADES").is_none());

        // Reuse the maximize-then-replay measurement and floor grids from the
        // Pareto library so the CSV sweep and `--solve` can't drift apart. The
        // sweep emits every raw sample (the Python plotter derives its own
        // frontier), so it calls `measure_point` per floor rather than
        // `solve_pareto`, which would only return frontier points.
        use solver::decomp_stats;
        use solver::pareto::{
            measure_point_chained, FrontierKind, DEFENSE_FLOORS, STABILITY_FLOORS,
        };

        let report = |name: &str,
                      kind: &str,
                      floor: f64,
                      point: Option<&solver::pareto::ParetoPoint>,
                      secs: f64| {
            if let Some(p) = point {
                println!(
                    "{name},{kind},{:.0},{:.1},{:.3},{:.3}",
                    p.floor, p.income, p.stability, p.defense
                );
            }
            if show_stats {
                eprintln!("STATS {name} {kind} {floor:.0} {secs:.2}s");
            }
        };

        for name in names {
            let system = &systems[name];
            decomp_stats::reset();
            let system_start = std::time::Instant::now();

            let mut stability_warm = None;
            let first_stability = STABILITY_FLOORS[0];
            let floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(first_stability));
            let t0 = std::time::Instant::now();
            let first_point = measure_point_chained(
                system,
                &base_balance,
                FrontierKind::Stability,
                first_stability as f64,
                &floors,
                horizon,
                time_limit,
                settings,
                &mut stability_warm,
            );
            let first_elapsed = t0.elapsed().as_secs_f64();
            let defense_initial_warm = stability_warm.clone();

            let stability_system = system.clone();
            let stability_balance = base_balance.clone();
            let defense_system = system.clone();
            let defense_balance = base_balance.clone();
            let (stability_tail, defense_points) = std::thread::scope(|scope| {
                let stability_handle = scope.spawn(move || {
                    cpu_affinity::prefer_performance_cores();

                    let mut warm = stability_warm;
                    let mut points = Vec::new();
                    for stab in STABILITY_FLOORS.iter().copied().skip(1) {
                        let floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(stab));
                        let t0 = std::time::Instant::now();
                        let point = measure_point_chained(
                            &stability_system,
                            &stability_balance,
                            FrontierKind::Stability,
                            stab as f64,
                            &floors,
                            horizon,
                            time_limit,
                            settings,
                            &mut warm,
                        );
                        points.push((stab as f64, point, t0.elapsed().as_secs_f64()));
                    }
                    points
                });

                let defense_handle = scope.spawn(move || {
                    cpu_affinity::prefer_performance_cores();

                    let mut warm = defense_initial_warm;
                    let mut points = Vec::new();
                    for def_floor in DEFENSE_FLOORS {
                        let floors = Goal::new(f64::NEG_INFINITY, Some(def_floor), Some(0));
                        let t0 = std::time::Instant::now();
                        let point = measure_point_chained(
                            &defense_system,
                            &defense_balance,
                            FrontierKind::Defense,
                            def_floor,
                            &floors,
                            horizon,
                            time_limit,
                            settings,
                            &mut warm,
                        );
                        points.push((def_floor, point, t0.elapsed().as_secs_f64()));
                    }
                    points
                });

                (
                    stability_handle.join().expect("stability chain panicked"),
                    defense_handle.join().expect("defense chain panicked"),
                )
            });

            report(
                name,
                "stability",
                first_stability as f64,
                first_point.as_ref(),
                first_elapsed,
            );
            for (floor, point, secs) in stability_tail {
                report(name, "stability", floor, point.as_ref(), secs);
            }
            for (floor, point, secs) in defense_points {
                report(name, "defense", floor, point.as_ref(), secs);
            }

            if show_stats {
                let total_elapsed = system_start.elapsed().as_secs_f64();
                let (calls, hits, steps, cands, score_passes, reuse_steps, seeds) =
                    decomp_stats::snapshot();
                eprintln!(
                    "STATS-TOTAL {name} {total_elapsed:.2}s run_plan={calls} cache_hits={hits} sim_steps={steps} cand_scores={cands} score_passes={score_passes} reuse_steps={reuse_steps} seeds={seeds}"
                );
            }
        }
        return Ok(());
    }

    let cli = Cli::parse();
    let settings = SolverSettings {
        include_industry_upgrades: cli.include_industry_upgrades,
        allow_parallel_builds: cli.parallel_builds,
    };

    if let Some(command) = cli.command {
        match command {
            Command::Extract(command) => {
                if let Err(err) = extract::cli::run(command) {
                    eprintln!("{err}");
                    std::process::exit(1);
                }
            }
            Command::Locate { starsector_dir } => {
                if let Err(err) = run_locate(starsector_dir) {
                    eprintln!("{err}");
                    std::process::exit(1);
                }
            }
            Command::Init {
                starsector_dir,
                save,
                latest,
                db,
            } => {
                if let Err(err) = run_init(starsector_dir, save, latest, db) {
                    eprintln!("{err}");
                    std::process::exit(1);
                }
            }
            Command::Inspect {
                db,
                save,
                all,
                systems,
            } => {
                inspect_systems(&db, save.as_deref(), all, &systems)?;
            }
            Command::Tui { starsector_dir } => {
                if let Err(err) = tui::run(starsector_dir) {
                    eprintln!("{err}");
                    std::process::exit(1);
                }
            }
        }
        return Ok(());
    }

    // stderr: `--rank --rank-csv` writes machine-readable CSV to stdout.
    eprintln!("Loading game data...");
    let systems = parser::load_game_data_from_db(&cli.db, cli.save.as_deref())?;

    let mut initial_balance = Balance::new(cli.credits, cli.story_points, cli.alpha_cores);
    for item in cli.items {
        initial_balance.add_colony_item(item);
    }

    if cli.rank {
        run_rank(
            &systems,
            &initial_balance,
            &cli.db,
            cli.save.as_deref(),
            &cli.rank_systems,
            cli.rank_scope,
            cli.discovery_definition,
            cli.include_core_worlds,
            cli.horizon,
            cli.time_limit,
            cli.rank_csv,
            cli.rank_scorer,
            cli.rank_sort,
            settings,
        );
        return Ok(());
    }

    let test_system = systems
        .get(&cli.system)
        .unwrap_or_else(|| {
            let mut names: Vec<&String> = systems.keys().collect();
            names.sort();
            eprintln!(
                "error: system \"{}\" not found. Available systems:",
                cli.system
            );
            for n in &names {
                eprintln!("  {n}");
            }
            std::process::exit(1);
        })
        .clone();

    let state = State::new(initial_balance, test_system);

    if cli.solve {
        run_solve(
            &cli.system,
            state.system(),
            state.balance(),
            cli.horizon,
            cli.time_limit,
            settings,
        );
        return Ok(());
    }

    if let Some(metric_str) = &cli.maximize {
        let metric = match metric_str.as_str() {
            "income" => Metric::Income,
            "defense" => Metric::Defense,
            "stability" => Metric::Stability,
            other => unreachable!("clap restricts --maximize values; got {other}"),
        };

        // Effective floors for the non-maximized metrics. Defaults when the
        // matching flag is omitted: income break-even (0), stability 7, defense 0.
        // The maximized metric itself is left unconstrained (it's what we push up).
        let income_floor = cli.income.unwrap_or(0.0);
        let stability_floor = cli.stability.unwrap_or(7);
        let defense_floor = cli.defense.unwrap_or(0.0);
        let floors = match metric {
            Metric::Income => Goal::new(
                f64::NEG_INFINITY,
                Some(defense_floor),
                Some(stability_floor),
            ),
            Metric::Defense => Goal::new(income_floor, None, Some(stability_floor)),
            Metric::Stability => Goal::new(income_floor, Some(defense_floor), None),
        };

        let mut floor_parts = Vec::new();
        if metric != Metric::Income {
            floor_parts.push(format!("income >= {income_floor:.0}"));
        }
        if metric != Metric::Stability {
            floor_parts.push(format!("stability >= {stability_floor}"));
        }
        if metric != Metric::Defense {
            floor_parts.push(format!("defense >= {defense_floor:.0}"));
        }
        let floors_desc = floor_parts.join(", ");
        println!(
            "Maximize {} within {} months ({})",
            metric.as_str(),
            cli.horizon,
            floors_desc,
        );

        test_maximize(
            state,
            metric,
            &floors,
            cli.horizon,
            cli.time_limit,
            settings,
        );
        return Ok(());
    }

    let income = cli.income.unwrap_or(200_000.0);
    let goal = Goal::new(income, cli.defense, cli.stability);

    println!(
        "Goal: income >= {:.0}{}{}",
        income,
        cli.stability
            .map_or(String::new(), |s| format!(", stability >= {s}")),
        cli.defense
            .map_or(String::new(), |d| format!(", defense >= {d}")),
    );

    test_solver(state, &goal, cli.time_limit, settings);
    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn tui_subcommand_accepts_starsector_dir() {
        let cli = Cli::parse_from([
            "system_solver",
            "tui",
            "--starsector-dir",
            r"C:\Games\Starsector",
        ]);

        match cli.command {
            Some(Command::Tui {
                starsector_dir: Some(path),
            }) => assert_eq!(path, PathBuf::from(r"C:\Games\Starsector")),
            other => panic!("expected tui --starsector-dir, got {other:?}"),
        }
    }

    #[test]
    fn locate_subcommand_accepts_no_args() {
        let cli = Cli::parse_from(["system_solver", "locate"]);

        match cli.command {
            Some(Command::Locate {
                starsector_dir: None,
            }) => {}
            other => panic!("expected locate command, got {other:?}"),
        }
    }

    #[test]
    fn locate_subcommand_accepts_starsector_dir() {
        let cli = Cli::parse_from([
            "system_solver",
            "locate",
            "--starsector-dir",
            r"C:\Games\Starsector",
        ]);

        match cli.command {
            Some(Command::Locate {
                starsector_dir: Some(path),
            }) => assert_eq!(path, PathBuf::from(r"C:\Games\Starsector")),
            other => panic!("expected locate --starsector-dir, got {other:?}"),
        }
    }

    #[test]
    fn init_subcommand_accepts_no_args() {
        let cli = Cli::parse_from(["system_solver", "init"]);

        match cli.command {
            Some(Command::Init {
                starsector_dir: None,
                save: None,
                latest: false,
                db: None,
            }) => {}
            other => panic!("expected init command, got {other:?}"),
        }
    }

    #[test]
    fn init_subcommand_accepts_options() {
        let cli = Cli::parse_from([
            "system_solver",
            "init",
            "--starsector-dir",
            r"C:\Games\Starsector",
            "--save",
            "DEMIURGE",
            "--latest",
            "--db",
            "workspace/test.db",
        ]);

        match cli.command {
            Some(Command::Init {
                starsector_dir: Some(starsector_dir),
                save: Some(save),
                latest: true,
                db: Some(db),
            }) => {
                assert_eq!(starsector_dir, PathBuf::from(r"C:\Games\Starsector"));
                assert_eq!(save, "DEMIURGE");
                assert_eq!(db, PathBuf::from("workspace/test.db"));
            }
            other => panic!("expected init command with options, got {other:?}"),
        }
    }

    #[test]
    fn rank_accepts_discovery_scope_and_sort_options() {
        let cli = Cli::parse_from([
            "system_solver",
            "--rank",
            "--rank-scope",
            "discovered",
            "--discovery-definition",
            "fully-surveyed",
            "--include-core-worlds",
            "--rank-sort",
            "total-score",
        ]);

        assert!(cli.rank);
        assert_eq!(cli.rank_scope, RankScope::Discovered);
        assert_eq!(cli.discovery_definition, DiscoveryDefinition::FullySurveyed);
        assert!(cli.include_core_worlds);
        assert_eq!(cli.rank_sort, RankSortMode::TotalScore);
    }

    #[test]
    fn inspect_subcommand_accepts_system_filters() {
        let cli = Cli::parse_from([
            "system_solver",
            "inspect",
            "--db",
            "save_data.db",
            "--save",
            "latest",
            "Moloch",
            "Haojing",
        ]);

        match cli.command {
            Some(Command::Inspect {
                db,
                save: Some(save),
                all: false,
                systems,
            }) => {
                assert_eq!(db, PathBuf::from("save_data.db"));
                assert_eq!(save, "latest");
                assert_eq!(systems, vec!["Moloch", "Haojing"]);
            }
            other => panic!("expected inspect command, got {other:?}"),
        }
    }
}
