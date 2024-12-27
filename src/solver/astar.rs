use std::collections::{BinaryHeap, HashMap, HashSet};
use std::cmp::Ordering;
use std::hash::BuildHasherDefault;
use std::time::{Duration, Instant};
use nohash_hasher::NoHashHasher;
use rayon::prelude::*;
use crate::solver::{Action, Balance, SearchInfo, State, SearchResult, state::get_action_sequence_hash};
use crate::planet::Planet;
use crate::system::System;

#[derive(Debug, Clone)]
pub struct Goal {
    min_net_income: f64,
    min_ground_defense: Option<f64>,
    min_stability: Option<i32>,
}


impl Goal {
    pub fn new(min_net_income: f64, min_ground_defense: Option<f64>, min_stability: Option<i32>) -> Self {
        Self {
            min_net_income,
            min_ground_defense,
            min_stability,
        }
    }

    pub fn is_satisfied(&self, state: &State) -> bool {
        if state.balance().net_income() < self.min_net_income {
            return false;
        }

        let system = state.system();

        if let Some(min_defense) = self.min_ground_defense {
            if system.avg_ground_defense() < min_defense {
                return false;
            }
        }

        if let Some(min_stability) = self.min_stability {
            if system.avg_stability() < min_stability as f64 {
                return false;
            }
        }

        true
    }

    pub fn heuristic(&self, state: &State) -> i32 {
        let net_income_diff = (self.min_net_income - state.balance().net_income()).max(0.0);
        let defense_diff = self.min_ground_defense.map_or(0.0, |min_defense| (min_defense - state.system().avg_ground_defense()).max(0.0));
        let stability_diff = self.min_stability.map_or(0.0, |min_stability| (min_stability as f64 - state.system().avg_stability()).max(0.0));
        
        let max_value = net_income_diff.max(defense_diff).max(stability_diff);
        
        if max_value == 0.0 {
            return 0;
        }
        
        ((net_income_diff / max_value * net_income_diff) +
         (defense_diff / max_value * defense_diff) +
         (stability_diff / max_value * stability_diff)) as i32
    }
}


#[derive(Debug, Clone)]
pub struct AStarSearchResult {
    pub solution: Option<Vec<Action>>,
    pub cost: i32,
    pub cutoff_occurred: bool,
}

fn action_cost(action: &Action) -> i32 {
    // TODO: Use opportunity cost and make it depend on the action
    match action {
        Action::Wait(months) => *months as i32,
        _ => 0,
    }
}

fn ida_star(initial_state: &mut State, goal: &Goal, time_limit: u32, slim: bool) -> Option<AStarSearchResult> {
    println!("Starting IDA* search with time limit: {} ms", time_limit);
    let start_time = Instant::now();
    let mut bound = goal.heuristic(initial_state);
    let mut visited: HashSet<u64, BuildHasherDefault<NoHashHasher<u64>>> = HashSet::with_hasher(BuildHasherDefault::default());
    
    loop {
        println!("Current bound: {:?}", bound);
        visited.clear();
        let result = depth_limited_search(initial_state, goal, 0, bound, &mut visited, slim);
        
        if result.solution.is_some() {
            println!("Solution found!");
            return Some(result);
        }
        
        if result.cutoff_occurred {
            println!("Cutoff occurred. Increasing bound to {:?}", result.cost);
            bound = result.cost;
        } else {
            println!("No solution found within current bound");
            return None;
        }
        
        if start_time.elapsed() > Duration::from_millis(time_limit as u64) {
            println!("Time limit of {} ms exceeded", time_limit);
            return None;
        }
    }
}

fn depth_limited_search(
    state: &mut State,
    goal: &Goal,
    g: i32,
    bound: i32,
    visited: &mut HashSet<u64, BuildHasherDefault<NoHashHasher<u64>>>,
    slim: bool
) -> AStarSearchResult {
    let f = g + goal.heuristic(state);
    
    if f > bound {
        return AStarSearchResult {
            solution: None,
            cost: f,
            cutoff_occurred: true,
        };
    }
    
    if goal.is_satisfied(state) {
        return AStarSearchResult {
            solution: Some(state.action_log().to_vec()),
            cost: g,
            cutoff_occurred: false,
        };
    }
    
    let mut current_actions = state.action_log().clone();
    let mut min = AStarSearchResult {
        solution: None,
        cost: i32::MAX,
        cutoff_occurred: false,
    };
    
    for action in state.get_ordered_possible_actions(slim) {
        current_actions.push(action.clone());
        let next_hash = get_action_sequence_hash(&current_actions);
        current_actions.pop();
        
        if !visited.insert(next_hash) {
            continue;
        }
        
        state.apply_action_raw(&action, false);
        let cost = action_cost(&action);
        let result = depth_limited_search(state, goal, g + cost, bound, visited, slim);
        state.undo_last_action(false);
        
        if result.solution.is_some() {
            return result;
        }
        
        if result.cutoff_occurred {
            min.cutoff_occurred = true;
            min.cost = min.cost.min(result.cost);
        } else if result.cost < min.cost {
            min.cost = result.cost;
        }
    }
    
    min
}

pub fn search_all_planets(initial_state: &mut State, goal: &Goal, time_limit: u32, slim: bool) -> Vec<AStarSearchResult> {
    // Split state into per-planet states
    let planet_states = initial_state.to_vec_by_planet();
    
    // Search each planet in parallel
    planet_states.into_par_iter()
        .filter_map(|mut state| ida_star(&mut state, goal, time_limit, slim))
        .collect()
}


/*
TODOS:
- Precalculate the maximum gross income, defense, and stability bonuses for each facility
- Use that with the state in the heuristic function to calculate the minimum possible months to wait
    to achieve the goal
- Maybe still cost non-wait actions based on their construction time?
*/