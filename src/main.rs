mod constants;
mod cpu_affinity;
mod extract;
mod parser;
mod planet;
mod rank;
mod solve;
mod solver;
mod system;
mod tui;
mod utils;

use clap::{Parser, Subcommand};
use planet::Planet;
use rank::{filter_system_names, peak_income, rank_systems, score_per_planet, RankScorer};
use solver::{
    diagnose_maximize_gap, search_system_decomp, search_system_maximize, solve_pareto, Balance,
    Goal, Metric, State,
};
use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
// Archived solvers, kept reachable for benchmarking/comparison:
use constants::{ColonyItem, FacilityType};
use solver::archive::{astar::search_all_planets, split::search_all_planets_decomp};
use solver::Action;
use system::System;

#[derive(Parser)]
#[command(about = "Starsector colony system solver")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Star system to solve (name as extracted into the DB; see `extract search`)
    #[arg(long, default_value = "Mia's Star")]
    system: String,

    /// Extraction DB to load game data from (produced by `extract run`)
    #[arg(long, default_value = "save_data.db")]
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

    /// Colony item to start with (repeatable, e.g. --item "corrupted nanoforge")
    #[arg(long = "item")]
    items: Vec<ColonyItem>,

    /// Solver time budget in milliseconds
    #[arg(long, default_value_t = 25_000)]
    time_limit: u32,

    /// Rank systems by quick Pareto score (sparse floors, reduced search; see
    /// QUICK_RANKING_DESIGN.md). Ranks every system in the DB unless
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
    /// QUICK_RANKING_DESIGN.md.
    #[arg(long = "rank-scorer", value_enum, default_value_t = RankScorer::Quick)]
    rank_scorer: RankScorer,

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
    /// Open the interactive terminal UI.
    Tui {
        /// Starsector install directory. Overrides solver_tui.toml for this run.
        #[arg(long)]
        starsector_dir: Option<PathBuf>,
    },
}

/// Load solver game data from the extraction DB. Env-var modes use
/// `SYSTEM_SOLVER_DB` / `SYSTEM_SOLVER_SAVE` since they bypass CLI parsing.
fn load_systems_from_env() -> Result<HashMap<String, System>, Box<dyn Error>> {
    let db = std::env::var("SYSTEM_SOLVER_DB").unwrap_or_else(|_| "save_data.db".to_string());
    let save = std::env::var("SYSTEM_SOLVER_SAVE").ok();
    Ok(parser::load_game_data_from_db(&db, save.as_deref())?)
}

/// Run the maximize-mode solver and report the best plan plus the metric values
/// it actually achieves (by replaying the solution onto a fresh copy of `state`).
fn test_maximize(
    mut state: State,
    metric: Metric,
    floors: &Goal,
    horizon: i32,
    time_limit: u32,
    include_industry_upgrades: bool,
) {
    println!(
        "\nStarting maximize solver ({}, horizon {} months)...",
        metric.as_str(),
        horizon
    );

    let replay_base = state.clone();
    let results = search_system_maximize(
        &mut state,
        metric,
        floors,
        horizon,
        time_limit,
        !include_industry_upgrades,
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

fn test_solver(mut state: State, goal: &Goal, time_limit: u32, include_industry_upgrades: bool) {
    println!("\nStarting solver test...");
    println!("Initial state score: {}", state.score() as i32);

    // Default solver: the joint two-level decomposition (shared timeline).
    let results = search_system_decomp(&mut state, goal, time_limit, !include_industry_upgrades);

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
    include_industry_upgrades: bool,
) {
    println!("Pareto solve for {system_name} (horizon {horizon} months)");
    println!(
        "Starting balance: credits={:.0}, story_points={}, alpha_cores={}",
        balance.credits(),
        balance.story_points(),
        balance.alpha_cores(),
    );

    let solution = solve_pareto(
        system,
        balance,
        horizon,
        time_limit,
        include_industry_upgrades,
    );

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
/// QUICK_RANKING_DESIGN.md.
fn run_rank(
    systems: &HashMap<String, System>,
    balance: &Balance,
    filters: &[String],
    horizon: i32,
    time_limit: u32,
    csv: bool,
    scorer: RankScorer,
    include_industry_upgrades: bool,
) {
    let names = filter_system_names(systems, filters).unwrap_or_else(|_| {
        eprintln!("error: no system matches the --rank-system filter(s)");
        std::process::exit(1);
    });

    eprintln!(
        "Ranking {} systems ({scorer:?} scorer, horizon {horizon} months)...",
        names.len()
    );

    let ranked = rank_systems(
        systems,
        balance,
        &names,
        horizon,
        time_limit,
        scorer,
        include_industry_upgrades,
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

/// Build the standard A/B starting balance: generous credits plus a couple of
/// colony items, matching the default `main` setup so the comparison is
/// representative.
fn ab_balance() -> Balance {
    let mut balance = Balance::new(5_000_000.0, 5, 1);
    balance.add_colony_item(ColonyItem::CorruptedNanoforge);
    balance.add_colony_item(ColonyItem::SoilNanites);
    balance
}

/// Summarize a solver's per-planet results into (solved, best_months, all_costs).
fn summarize(results: &[solver::AStarSearchResult]) -> (usize, Option<i32>, Vec<i32>) {
    let costs: Vec<i32> = results.iter().map(|r| r.cost).collect();
    let best = costs.iter().copied().min();
    (results.len(), best, costs)
}

/// Run both solvers over every loaded system at an equal per-planet time budget
/// and print a side-by-side comparison. Triggered by `SYSTEM_SOLVER_AB=1`.
fn run_ab(systems: &HashMap<String, System>, budget_ms: u32) {
    use std::time::Instant;

    // A few goals spanning easy -> hard so we can see where each solver wins.
    let goals = [
        ("income>=10k, stab>=6", Goal::new(10_000.0, None, Some(6))),
        ("income>=40k, stab>=8", Goal::new(40_000.0, None, Some(8))),
        ("income>=80k, stab>=8", Goal::new(80_000.0, None, Some(8))),
    ];

    let mut names: Vec<&String> = systems.keys().collect();
    names.sort();

    println!("\n================ A/B: IDA* vs two-level decomposition ================");
    println!("per-planet budget: {} ms\n", budget_ms);

    for name in names {
        let system = &systems[name];
        let planet_count = system.planets().len();
        println!("### {name} ({planet_count} planets)");

        for (label, goal) in &goals {
            // Fresh state per solver so neither sees the other's mutations.
            let mut ida_state = State::new(ab_balance(), system.clone());
            let mut dec_state = State::new(ab_balance(), system.clone());

            let t0 = Instant::now();
            let ida = search_all_planets(&mut ida_state, goal, budget_ms, true);
            let ida_ms = t0.elapsed().as_millis();

            let t1 = Instant::now();
            let dec = search_all_planets_decomp(&mut dec_state, goal, budget_ms, true);
            let dec_ms = t1.elapsed().as_millis();

            let mut joint_state = State::new(ab_balance(), system.clone());
            let t2 = Instant::now();
            let joint = search_system_decomp(&mut joint_state, goal, budget_ms, true);
            let joint_ms = t2.elapsed().as_millis();

            let (ida_solved, ida_best, ida_costs) = summarize(&ida);
            let (dec_solved, dec_best, dec_costs) = summarize(&dec);
            let joint_cost = joint.first().map(|r| r.cost);

            let fmt_best = |b: Option<i32>| b.map_or("--".to_string(), |m| format!("{m}mo"));

            // NOTE: IDA* and per-planet decomp force *each* planet to meet the
            // threshold alone; joint solves the true system-wide goal (totals
            // across planets), so its cost is not directly comparable.
            println!("  goal: {label}");
            println!(
                "    IDA*         : solved {ida_solved}/{planet_count}  best {:>5}  {:>7}ms  costs {:?}",
                fmt_best(ida_best),
                ida_ms,
                ida_costs
            );
            println!(
                "    decomp/split : solved {dec_solved}/{planet_count}  best {:>5}  {:>7}ms  costs {:?}",
                fmt_best(dec_best),
                dec_ms,
                dec_costs
            );
            println!(
                "    decomp/joint : {}  {:>7}ms  (system-wide goal)",
                joint_cost.map_or("unsolved".to_string(), |c| format!("solved cost {c}mo")),
                joint_ms,
            );
        }
        println!();
    }
    println!("=====================================================================\n");
}

/// Independently verify the joint decomp solver: solve the whole system, then
/// replay the returned log on a *fresh* copy of the full system and report the
/// recomputed net income, stability, unfinished facilities, and — crucially —
/// whether any one-off resource (alpha cores / story points) went negative,
/// which would mean a shared resource was double-spent. Trusts nothing about the
/// simulator's incremental bookkeeping.
fn verify_decomp(systems: &HashMap<String, System>) {
    let goal = Goal::new(40_000.0, None, Some(6));
    let mut names: Vec<&String> = systems.keys().collect();
    names.sort();

    println!("\n--------- joint decomp verification (goal income>=40k, stab>=6) ---------");
    for name in names {
        let mut state = State::new(ab_balance(), systems[name].clone());
        let planet_count = state.system().planets().len();

        let Some(result) = solver::decomp::decomp_search(&mut state, &goal, 50000, true) else {
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
        include_upgrades: bool,
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
            include_upgrades,
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
            include_upgrades,
            warm_bound,
            &cross,
            SearchProfile::FULL,
        );
        (greedy, bound)
    }

    let include_upgrades = std::env::var_os("SYSTEM_SOLVER_NO_UPGRADES").is_none();

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
            include_upgrades,
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
                            include_upgrades,
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
                            include_upgrades,
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

    if std::env::var_os("SYSTEM_SOLVER_AB").is_some() {
        println!("Loading game data...");
        let systems = load_systems_from_env()?;
        let budget_ms = std::env::var("SYSTEM_SOLVER_AB_MS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(8_000);
        run_ab(&systems, budget_ms);
        return Ok(());
    }

    if std::env::var_os("SYSTEM_SOLVER_VERIFY").is_some() {
        println!("Loading game data...");
        let systems = load_systems_from_env()?;
        verify_decomp(&systems);
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
        let include_upgrades = std::env::var_os("SYSTEM_SOLVER_NO_UPGRADES").is_none();

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
                include_upgrades,
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
                            include_upgrades,
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
                            include_upgrades,
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

    if let Some(command) = cli.command {
        match command {
            Command::Extract(command) => {
                if let Err(err) = extract::cli::run(command) {
                    eprintln!("{err}");
                    std::process::exit(1);
                }
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
            &cli.rank_systems,
            cli.horizon,
            cli.time_limit,
            cli.rank_csv,
            cli.rank_scorer,
            cli.include_industry_upgrades,
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
            cli.include_industry_upgrades,
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
            cli.include_industry_upgrades,
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

    test_solver(state, &goal, cli.time_limit, cli.include_industry_upgrades);
    return Ok(());

    // Test growth update
    // Apply actions to set up the initial state
    let terran_1_hash = 1160120806187968324; //Planet::_get_planet_name_hash("Terran 1");
    let action_sequence = vec![
        Action::Colonize(terran_1_hash),
        Action::Wait(1),
        Action::UpgradeAdmin(terran_1_hash),
        Action::AddFacility(terran_1_hash, FacilityType::Megaport),
        Action::AddFacility(terran_1_hash, FacilityType::LightIndustry),
        Action::AddFacility(terran_1_hash, FacilityType::GroundDefenses),
        Action::AddFacility(terran_1_hash, FacilityType::OrbitalStation),
        Action::SetHazardPay(terran_1_hash, true),
        Action::SetFreePort(terran_1_hash, true),
        Action::Wait(3),
        Action::InstallItem(
            terran_1_hash,
            FacilityType::LightIndustry,
            ColonyItem::BiofactoryEmbryo,
        ),
        Action::Wait(2),
    ];

    let test_action_sequence = vec![
        // Action::AddFacility(terran_1_hash, FacilityType::LightIndustry),
        // Action::Wait(10),
        // Action::AddFacility(terran_1_hash, FacilityType::HeavyIndustry),
        // Action::Wait(10),
    ];

    println!("\nInitial state:");
    println!("{:#?}", state.balance());

    // Apply action sequence
    for action in &action_sequence {
        state.apply_action_raw(action, true);
    }

    println!("\nState after applying action sequence:");
    println!("{:#?}", state.balance());

    let initial_credits = state.balance().credits();

    // Apply test actions
    for action in &test_action_sequence {
        state.apply_action_raw(action, true);
    }

    // Undo test actions
    for _ in 0..test_action_sequence.len() {
        state.undo_last_action(true);
    }

    println!("\nState after undoing test actions:");
    println!("{:#?}", state.balance());

    // Check for credit inconsistency
    let final_credits = state.balance().credits();
    let credit_difference = final_credits - initial_credits;
    println!("\nCredit difference: {}", credit_difference);

    if credit_difference.abs() > 1e-6 {
        println!("Warning: Credit inconsistency detected!");
    } else {
        println!("No credit inconsistency detected.");
    }

    crate::solver::_test_path_undo_consistency(&state);

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
}

/*
TODOS:
- Make everything hashable - Done
- Add State struct for system info - Done
- Add new actions for waiting a number of months and colonizing a planet - Done
- Add corresponding functions for the system impl - Done
- Add a bank/balance struct to keep track of credits, SPs, and alpha cores - Done
- Give harvested organs the same treatment as drugs for production functions - Done
- Add income functions to facilities and planets - Done
- Add score function to system/state structs - Done
- Hash action sequences instead of system state - Done
- Add the ability to undo any action so we can efficiently search the game tree - Done
- Get a DFS working - Doneish
- Redo the upkeep/production formulas to be a function pointer - Done

- Rework facility upgrading/downgrading to do less reallocation
- Rework lazy static hashmaps in constants file to be match statements
- Implement system-wide restrictions on commerce facility
- Fix colony growth and reversability
- Search in two or more phases, first without any facility improvements, then use those action logs to help the full search
- ^ Could also search without any defenseive structures first, then try to fit them in later
- Search one planet at a time, and then plug in the results of each planet into the overall search tree at the end
- Use a different search algorithm, like IDA* with heuristics
- Let the user give a specific goal (net income and/or credits, defense multiplier) to optimize for


Since the search space for the full tree is quintillions of nodes the plan to do things in stages:
1. Search each planet individually using state.to_vec_by_planet; in 'exclude_upgrades' mode to limit the search space; this should be parallelizable
2. Run a quick algorithm to insert AddImprovement and AddAlphaCore actions into the sequence where they'd be most effective, for each optimal action sequence returned from the exclude_upgrades searches
3. Use these near-optimal action sequences and other data from each individual planet to do one big combined search on the full tree relying on the heuristic data gathered to traverse it quickly and find the optimal solution

*/
