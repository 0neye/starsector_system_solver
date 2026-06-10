//! Archived score-maximizing search: the original iterative-deepening DFS
//! (`search`/`dfs`) and the greedy `simulate_linear` driver, predating the
//! goal-directed solvers. Kept for reference; not on the default path.

use std::collections::HashSet;
use std::time::Instant;

use nohash_hasher::BuildNoHashHasher;

use crate::solver::state::{get_action_sequence_hash, Action, State};

#[derive(Debug, Clone)]
pub struct SearchInfo {
    state: State,
    start_time: Instant,
    time_limit: u32, // in milliseconds
}

impl SearchInfo {
    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn action_log(&self) -> &Vec<Action> {
        self.state.action_log()
    }

    pub fn start_time(&self) -> Instant {
        self.start_time
    }

    pub fn time_limit(&self) -> u32 {
        self.time_limit
    }

    #[inline(always)]
    pub fn is_time_up(&self) -> bool {
        self.start_time.elapsed().as_millis() as u32 >= self.time_limit
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub score: f64,
    pub nodes_explored: u32,
    pub action_log: Vec<Action>,
}

const MAX_DEPTH: u32 = 100;

pub fn search(initial_state: &State, time_limit: u32, slim: bool) -> SearchResult {
    println!("Starting search with time limit: {} ms", time_limit);

    let mut info = SearchInfo {
        state: initial_state.clone(),
        start_time: std::time::Instant::now(),
        time_limit,
    };

    println!("Initial state score: {}", initial_state.score());

    let mut best_actions = Vec::new();
    let mut best_score = f64::NEG_INFINITY;
    let mut nodes_explored = 0;
    let mut last_print_time = std::time::Instant::now();
    let mut last_unique_nodes = 0;

    for depth in 2..=MAX_DEPTH {
        let starting_size = (2u32.pow(depth) as f64).powf(2.6);
        let mut tt: HashSet<u64, BuildNoHashHasher<u64>> =
            HashSet::with_capacity_and_hasher(starting_size as usize, BuildNoHashHasher::default());

        if info.is_time_up() {
            println!("Time limit reached at depth {}", depth);
            break;
        }

        let depth_start_time = std::time::Instant::now();
        println!("\nSearching at depth {}", depth);
        let result = dfs(&mut info, depth, best_score, &mut tt, slim);
        if result.is_none() {
            println!("Search interrupted at depth {}", depth);
            break;
        }

        let result = result.unwrap();
        let score = result.score;
        let depth_duration = depth_start_time.elapsed();
        println!("Depth {} completed in {:?}. Score: {}, Nodes explored: {}, Unique positions explored: {}", depth, depth_duration, score, result.nodes_explored, tt.len());

        let current_time = std::time::Instant::now();
        let elapsed = current_time.duration_since(last_print_time);
        if elapsed >= std::time::Duration::from_secs(1) {
            let unique_nodes_per_sec =
                (tt.len() as f64 - last_unique_nodes as f64) / elapsed.as_secs_f64();
            println!("Unique nodes/sec: {:.2}", unique_nodes_per_sec);
            last_print_time = current_time;
            last_unique_nodes = tt.len() as u32;
        }

        if score > best_score {
            best_score = score;
            best_actions = result.action_log;
            nodes_explored += result.nodes_explored;
            println!("New best score: {} at depth {}", best_score, depth);
        }
    }

    println!(
        "Search completed. Best score: {}, Total nodes explored: {}",
        best_score, nodes_explored
    );
    println!(
        "Ending State: {:#?}\n{:#?}",
        info.state.balance(),
        info.state.action_log()
    );
    SearchResult {
        score: best_score,
        nodes_explored,
        action_log: best_actions,
    }
}

fn dfs(
    info: &mut SearchInfo,
    depth: u32,
    alpha: f64,
    tt: &mut HashSet<u64, BuildNoHashHasher<u64>>,
    slim: bool,
) -> Option<SearchResult> {
    if info.is_time_up() {
        return None;
    }

    let mut result = SearchResult {
        score: 0.0,
        nodes_explored: 1,
        action_log: info.state.action_log().clone(),
    };

    if depth == 0 {
        result.score = info.state.score();
        return Some(result);
    }

    let actions = info.state.get_ordered_possible_actions(slim);
    let mut orig_action_log = info.state.action_log().clone();
    let mut best_action_log = info.state.action_log().clone();
    let mut best_score = alpha;

    for action in actions.iter() {
        result.nodes_explored += 1;
        orig_action_log.push(action.clone());
        let next_hash = get_action_sequence_hash(&orig_action_log);
        orig_action_log.pop();

        if !tt.insert(next_hash) {
            continue;
        }

        info.state.apply_action_raw(action, false);

        let sub_result = dfs(info, depth - 1, best_score, tt, slim);

        if let Some(sub_result) = sub_result {
            result.nodes_explored += sub_result.nodes_explored;
            if sub_result.score > best_score {
                best_score = sub_result.score;
                best_action_log = sub_result.action_log.clone();
            }
        } else {
            info.state.undo_last_action(false);
            return None;
        }

        info.state.undo_last_action(false);
    }

    result.score = best_score;
    result.action_log = best_action_log;

    Some(result)
}

pub fn simulate_linear(initial_state: &State, num_turns: u32) -> SearchInfo {
    let mut info = SearchInfo {
        state: initial_state.clone(),
        start_time: std::time::Instant::now(),
        time_limit: 5000,
    };

    println!("\nStarting linear simulation for {} turns...", num_turns);
    println!("Initial state score: {}", info.state.score());
    println!("Initial credits: {}", info.state.balance().credits() as i32);

    for turn in 0..num_turns {
        println!("\nTurn {}", turn + 1);
        println!("Credits: {}", info.state.balance().credits() as i32);

        let actions = info.state.get_ordered_possible_actions(true);
        if actions.is_empty() {
            println!("No valid actions available!");
            break;
        }

        let mut best_score = f64::NEG_INFINITY;
        let mut best_action = None;
        let mut best_next_state = None;

        for action in actions {
            let mut next_state = info.state.clone();
            next_state.apply_action_raw(&action, false);
            let score = next_state.score();

            println!("  Action: {:?} -> Score: {}", action, score as i32);

            if score > best_score {
                best_score = score;
                best_action = Some(action);
                best_next_state = Some(next_state);
            }
        }

        if let (Some(action), Some(next_state)) = (best_action, best_next_state) {
            println!("Choosing action: {:?}", action);
            info.state = next_state;
        } else {
            println!("No valid action found!");
            break;
        }

        println!("\nColony Status:");
        for (name, planet) in info.state.system().planets().iter() {
            if planet.has_colony() {
                println!(
                    "\n  {} - Income: {} - Size: {}",
                    name,
                    planet.get_net_income(),
                    planet.size()
                );
                println!("    Facility Status:");
                for facility in planet.facilities().iter() {
                    let name = facility.name();
                    println!(
                        "    {} - Income: {} - Prod: {:#?}",
                        name,
                        facility.calculate_net_income(
                            planet.size(),
                            planet,
                            planet.calculate_accessibility()
                        ),
                        facility.get_resource_production(planet.size(), 0.0, planet.is_free_port())
                    );
                }
            }
        }
    }

    println!("\nAction sequence:");
    for action in info.action_log() {
        println!("  - {:?}", action);
    }

    println!("\nSimulation complete!");
    println!("Final score: {}", info.state.score());
    println!("Final credits: {}", info.state.balance().credits());

    for _ in 0..num_turns {
        info.state.undo_last_action(false);
    }

    info
}
