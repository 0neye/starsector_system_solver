mod constants;
mod utils;
mod planet;
mod system;
mod solver;
mod parser;

use std::error::Error;
use std::collections::HashMap;
use clap::Parser;
use planet::Planet;
use solver::{search_system_decomp, search_system_maximize, Goal, Metric, Balance, State};
// Archived solvers, kept reachable for benchmarking/comparison:
use solver::archive::{astar::search_all_planets, split::search_all_planets_decomp};
use constants::{ColonyItem, FacilityType};
use solver::Action;
use system::System;

#[derive(Parser)]
#[command(about = "Starsector colony system solver")]
struct Cli {
    /// Star system to solve (name as it appears in Planets.csv)
    #[arg(long, default_value = "Mia Bravos")]
    system: String,

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

    /// Starting credits
    #[arg(long, default_value_t = 5_000_000.0)]
    credits: f64,

    /// Starting story points
    #[arg(long, default_value_t = 5)]
    story_points: u32,

    /// Starting alpha cores
    #[arg(long, default_value_t = 1)]
    alpha_cores: u32,

    /// Colony item to start with (repeatable, e.g. --item "corrupted nanoforge")
    #[arg(long = "item")]
    items: Vec<ColonyItem>,

    /// Solver time budget in milliseconds
    #[arg(long, default_value_t = 25_000)]
    time_limit: u32,

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

/// Run the maximize-mode solver and report the best plan plus the metric values
/// it actually achieves (by replaying the solution onto a fresh copy of `state`).
fn test_maximize(
    mut state: State,
    metric: Metric,
    floors: &Goal,
    horizon: i32,
    time_limit: u32,
) {
    println!("\nStarting maximize solver ({}, horizon {} months)...", metric.as_str(), horizon);

    let replay_base = state.clone();
    let results = search_system_maximize(&mut state, metric, floors, horizon, time_limit, true);

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

fn test_solver(mut state: State, goal: &Goal, time_limit: u32) {
    println!("\nStarting solver test...");
    println!("Initial state score: {}", state.score() as i32);

    // Default solver: the joint two-level decomposition (shared timeline).
    let results = search_system_decomp(&mut state, goal, time_limit, true);

    if results.is_empty() {
        println!("No solution found within time limit.");
    } else {
        // Print all results found
        for (i, result) in results.iter().enumerate() {
            println!("Result {}:\n{:#?}", i + 1, result);
        }
    }
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

fn main() -> Result<(), Box<dyn Error>> {
    // Special env-var modes bypass normal CLI parsing.
    if std::env::var_os("SYSTEM_SOLVER_AB").is_some() {
        println!("Loading game data...");
        let systems = parser::load_game_data("Planets.csv", "Infrastructure.csv")?;
        let budget_ms = std::env::var("SYSTEM_SOLVER_AB_MS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(8_000);
        run_ab(&systems, budget_ms);
        return Ok(());
    }

    if std::env::var_os("SYSTEM_SOLVER_VERIFY").is_some() {
        println!("Loading game data...");
        let systems = parser::load_game_data("Planets.csv", "Infrastructure.csv")?;
        verify_decomp(&systems);
        return Ok(());
    }

    let cli = Cli::parse();

    println!("Loading game data...");
    let systems = parser::load_game_data("Planets.csv", "Infrastructure.csv")?;

    let test_system = systems
        .get(&cli.system)
        .unwrap_or_else(|| {
            let mut names: Vec<&String> = systems.keys().collect();
            names.sort();
            eprintln!("error: system \"{}\" not found. Available systems:", cli.system);
            for n in &names {
                eprintln!("  {n}");
            }
            std::process::exit(1);
        })
        .clone();

    let mut initial_balance = Balance::new(cli.credits, cli.story_points, cli.alpha_cores);
    for item in cli.items {
        initial_balance.add_colony_item(item);
    }

    let state = State::new(initial_balance, test_system);

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
            Metric::Income => {
                Goal::new(f64::NEG_INFINITY, Some(defense_floor), Some(stability_floor))
            }
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

        test_maximize(state, metric, &floors, cli.horizon, cli.time_limit);
        return Ok(());
    }

    let income = cli.income.unwrap_or(200_000.0);
    let goal = Goal::new(income, cli.defense, cli.stability);

    println!(
        "Goal: income >= {:.0}{}{}",
        income,
        cli.stability.map_or(String::new(), |s| format!(", stability >= {s}")),
        cli.defense.map_or(String::new(), |d| format!(", defense >= {d}")),
    );

    test_solver(state, &goal, cli.time_limit);
    return Ok(());

    // Test growth update
    // Apply actions to set up the initial state
    let terran_1_hash = 1160120806187968324;//Planet::_get_planet_name_hash("Terran 1");
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
        Action::InstallItem(terran_1_hash, FacilityType::LightIndustry, ColonyItem::BiofactoryEmbryo),
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
1. Search each planet individually using state.to_vec_by_planet; in 'slim' mode to limit the search space; this should be parallelizable
2. Run a quick algorithm to insert AddImprovement and AddAlphaCore actions into the sequence where they'd be most effective, for each optimal action sequence returned from the slim searches
3. Use these near-optimal action sequences and other data from each individual planet to do one big combined search on the full tree relying on the heuristic data gathered to traverse it quickly and find the optimal solution

*/