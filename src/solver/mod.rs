use crate::constants::{ColonyItem, AdminType, FACILITY_DATA};
use crate::system::{System};
use core::panic;
use std::collections::{hash_map, HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hash, Hasher};
use std::time::Instant;
use std::vec;

use nohash_hasher::{BuildNoHashHasher, NoHashHasher};

pub mod state;
pub mod astar;

use state::get_action_sequence_hash;
pub use state::{State, Balance, Action};

#[derive(Debug, Clone)]
pub struct SearchInfo {
    state: State,
    start_time: Instant,
    time_limit: u32,  // in milliseconds
}

impl SearchInfo {
    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn action_log(&self) -> &Vec<Action> {
        &self.state.action_log()
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
        let mut tt: HashSet<u64, BuildNoHashHasher<u64>> = HashSet::with_capacity_and_hasher(starting_size as usize, BuildNoHashHasher::default());

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
            let unique_nodes_per_sec = (tt.len() as f64 - last_unique_nodes as f64) / elapsed.as_secs_f64();
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
    
    println!("Search completed. Best score: {}, Total nodes explored: {}", best_score, nodes_explored);
    println!("Ending State: {:#?}\n{:#?}", info.state.balance(), info.state.action_log());
    SearchResult {
        score: best_score,
        nodes_explored,
        action_log: best_actions,
    }
}



fn dfs(info: &mut SearchInfo, depth: u32, alpha: f64, tt: &mut HashSet<u64, BuildNoHashHasher<u64>>, slim: bool) -> Option<SearchResult> {
    // let indent = " ".repeat((MAX_DEPTH - depth + 1) as usize);
    // println!("{}Entering dfs: depth={}, alpha={}", indent, depth, alpha);
    
    if info.is_time_up() {
        // println!("{}Time up, exiting dfs", indent);
        return None;
    }

    // let hash = info.state.get_hash();
    // if let Some(result) = tt.get(&hash) {
    //     // println!("{}Found in transposition table: {}", indent, result.score);
    //     return Some(result.clone());
    // }

    let mut result = SearchResult {
        score: 0.0,
        nodes_explored: 1,
        action_log: info.state.action_log().clone(),
    };

    if depth == 0 {
        // println!("{}Reached depth 0, calculating score", indent);
        result.score = info.state.score();
        // println!("{}Leaf node score: {}", indent, result.score);
        // _test_path_undo_consistency(&info.state);
        return Some(result);
    }

    // println!("{}Getting possible actions", indent);
    let actions = info.state.get_ordered_possible_actions(slim);
    // println!("{}Number of possible actions: {}", indent, actions.len());
    // println!("{}Actions: {:#?}", indent, actions);
    let mut orig_action_log = info.state.action_log().clone();
    let mut best_action_log = info.state.action_log().clone();
    let mut best_score = alpha;

    for action in actions.iter() {
        result.nodes_explored += 1;
        // let pre_action_credits = info.state.balance().credits();
        // println!("{}Applying action {} of {}: {:?}", indent, i + 1, actions.len(), action);
        // let mut orig_state = info.state.clone();
        orig_action_log.push(action.clone());
        let next_hash = get_action_sequence_hash(&orig_action_log);
        orig_action_log.pop();

        if !tt.insert(next_hash) {
            continue;
        }

        info.state.apply_action_raw(action, false);

        // println!("{}Recursive call: depth={}, best_score={}", indent, depth - 1, best_score);
        let sub_result = dfs(info, depth - 1, best_score, tt, slim);

        if let Some(sub_result) = sub_result {
            result.nodes_explored += sub_result.nodes_explored;
            // println!("{}Subresult score: {}", indent, sub_result.score);
            if sub_result.score > best_score {
                best_score = sub_result.score;
                best_action_log = sub_result.action_log.clone();
                // println!("{}New best score: {}", indent, best_score);
            }
        } else {
            // println!("{}Time up during recursive call, exiting", indent);
            info.state.undo_last_action(false);
            return None;
        }

        // println!("{}Undoing action", indent);
        info.state.undo_last_action(false);
        
        // let diffs = orig_state.system()._get_differences(&info.state.system());
        // if !diffs.is_empty() {
        //     orig_state.system()._print_differences(&info.state.system());
        //     panic!("State inconsistency detected");
        // }
        // if (orig_state.balance().credits() - info.state.balance().credits()).abs() > 1e-6 {
        //     println!("Credit inconsistency detected at depth {} and action {:?}. Original credits: {}; New credits: {}", depth, action, orig_state.balance().credits(), info.state.balance().credits());
            
        //     if let Action::Wait(months) = action {
        //         let orig_bal = orig_state.balance().credits();
        //         orig_state.apply_action_raw(&Action::Wait(*months), true);
            
        //         let next_best_action = orig_state.get_ordered_possible_actions(true)[0].clone();

        //         let wait_bal = orig_state.balance().credits();
        //         let wait_credits = wait_bal - orig_bal;
        //         orig_state.undo_last_action(true);
        //         let undo_credits = wait_bal - orig_state.balance().credits();
        //         println!("Waited for {} months. Started with {} credits. Waited {} credits. Undid {} credits. Difference: {}", months, orig_bal, wait_credits, undo_credits, wait_credits - undo_credits);
        //         println!("The next applied action would be {:?}.", next_best_action);
        //         panic!("Action undo inconsistency detected");
        //     }
        // }
    }

    result.score = best_score;
    result.action_log = best_action_log;

    // println!("{}Exiting dfs: best_score={}", indent, best_score);
    Some(result)
}

fn _fac_inconsistency(state1: &State, state2: &State) -> bool {
    for planet_name in state1.system().planets().keys() {
        let facilities1 = &state1.system().planets()[planet_name].facilities();
        let facilities2 = &state2.system().planets()[planet_name].facilities();

        if facilities1.len() != facilities2.len() {
            return true;
        }

        if !facilities1.iter().zip(facilities2.iter()).all(|(fac1, fac2)| fac1.name() == fac2.name()) {
            return true;
        }
    }
    false
}

pub fn _test_path_undo_consistency(state: &State) {
    let actions = state.action_log().clone();
    let mut temp_state = state.clone();

    for _ in 0..=actions.len() {
        temp_state.undo_last_action(false);
    }
    temp_state.balance_mut().set_credits(10000000.0);
    let blank_state = temp_state.clone();

    let mut state_map = vec![];

    for action in actions.clone() {
        temp_state.apply_action_raw(&action, false);
        state_map.push(temp_state.clone());
    }

    for i in (0..state_map.len()).rev() {
        let should_be = &state_map[i];
        let is = &temp_state;
        let mut issue = false;
        if _fac_inconsistency(should_be, is) {
            println!("\nInconsistency found at action {:?}", actions[i]);
            for planet_name in should_be.system().planets().keys() {
                println!("Planet: {}", planet_name);
                println!("Should be: {:#?}", should_be.system().planets().get(planet_name).unwrap().facilities().iter().map(|f| f.name()).collect::<Vec<_>>());
                println!("Is: {:#?}", is.system().planets().get(planet_name).unwrap().facilities().iter().map(|f| f.name()).collect::<Vec<_>>());
            }
            issue = true;
        }
        if should_be.balance().credits() != is.balance().credits() {
            println!("\nInconsistency found at action {:?} - Credits", actions[i]);
            println!("Should be: {}", should_be.balance().credits());
            println!("Is: {}", is.balance().credits());
            issue = true;
        }
        if issue {
            println!("\nBlank State - Credits: {}", blank_state.balance().credits());
            println!("Blank State - Facilities: {:?}", blank_state.system().planets().values().flat_map(|p| p.facilities().iter().map(|f| (f.name(), f.remaining_build_days()))).collect::<Vec<_>>());
            println!("Should be State - Credits: {}", should_be.balance().credits());
            println!("Should be State - Facilities: {:?}", should_be.system().planets().values().flat_map(|p| p.facilities().iter().map(|f| (f.name(), f.remaining_build_days()))).collect::<Vec<_>>());
            println!("Is State - Credits: {}", is.balance().credits());
            println!("Is State - Facilities: {:?}", is.system().planets().values().flat_map(|p| p.facilities().iter().map(|f| (f.name(), f.remaining_build_days()))).collect::<Vec<_>>());
            println!("Action log: {:?}", actions);
            panic!();
        }
        temp_state.undo_last_action(false);
    }
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
        
        // Get all possible actions
        let actions = info.state.get_ordered_possible_actions(true);
        if actions.is_empty() {
            println!("No valid actions available!");
            break;
        }
        
        // Try each action and pick the one that leads to highest immediate score
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
        
        // Apply the best action
        if let (Some(action), Some(next_state)) = (best_action, best_next_state) {
            println!("Choosing action: {:?}", action);
            info.state = next_state;
        } else {
            println!("No valid action found!");
            break;
        }
        
        // Print colony status
        println!("\nColony Status:");
        for (name, planet) in info.state.system().planets().iter() {
            if planet.has_colony() {
                println!("\n  {} - Income: {} - Size: {}", name, planet.get_net_income(), planet.size());
                println!("    Facility Status:");
                for facility in planet.facilities().iter() {
                    let name = facility.name();
                    println!("    {} - Income: {} - Prod: {:#?}", name, facility.calculate_net_income(planet.size(), planet, planet.calculate_accessibility()), facility.get_resource_production(planet.size(), 0.0, planet.is_free_port()));
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

    // Undo all actions to restore initial state before returning
    for _ in 0..num_turns {
        info.state.undo_last_action(false);
    }
    
    info
}