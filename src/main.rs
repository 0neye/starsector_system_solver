mod constants;
mod utils;
mod planet;
mod system;
mod solver;
mod parser;

use std::error::Error;
use std::collections::HashMap;
use solver::{astar::{search_all_planets, Goal}, search, simulate_linear, Balance, SearchInfo, State};
use constants::ColonyItem;
use solver::Action;
use system::System;

fn test_solver(mut state: State, goal: &Goal) {
    println!("\nStarting solver test...");
    println!("Initial state score: {}", state.score() as i32);
    
    // Run simulation for 10 turns
    // simulate_linear(&state, 15);

    println!("{:#?}", search(&state, 25000, true).action_log);
    // println!("{:#?}", search_all_planets(&mut state, goal, 500000, true).iter().next().unwrap());

}

fn main() -> Result<(), Box<dyn Error>> {
    println!("Loading game data...");
    
    let systems = parser::load_game_data(
        "Planets.csv",
        "Infrastructure.csv"
    )?;
    
    // Create a test case with a single system
    let mut test_system = systems.get("Mia Bravos").unwrap().clone();

    // Reduce the system to one planet (Terran 1)
    test_system.remove_planet("GasGiant 1");
    test_system.remove_planet("Barren 1");
    
    // Create initial balance with more resources
    let mut initial_balance = Balance::new(
        10000000.0,
        5,
        5,
    );
    
    // Add more colony items
    let mut colony_items = HashMap::new();
    colony_items.insert(ColonyItem::CorruptedNanoforge, 2);
    colony_items.insert(ColonyItem::SoilNanites, 2);
    colony_items.insert(ColonyItem::BiofactoryEmbryo, 1);
    for (item, count) in colony_items {
        for _ in 0..count {
            initial_balance.add_colony_item(item);
        }
    }
    
    // Create initial state
    let mut state = State::new(initial_balance, test_system);

    // Create goal
    let goal = Goal::new(20000.0, None, None);

    
    // Run solver test
    test_solver(state, &goal);
    return Ok(());

    // Test growth update
    // Apply actions to set up the initial state
    let action_sequence = vec![
        Action::Colonize(
            "Terran 1".to_string(),
        ),
        Action::AddFacility(
            "Terran 1".to_string(),
            "commerce".to_string(),
        ),
        Action::Wait(
            13,
        ),
        Action::UpgradeAdmin(
            "Terran 1".to_string(),
        ),
        Action::AddFacility(
            "Terran 1".to_string(),
            "megaport".to_string(),
        ),
        Action::SetHazardPay(
            "Terran 1".to_string(),
            true,
        ),
        Action::Wait(
            5,
        ),
    ];

    let test_action_sequence = vec![
        // Action::AddFacility("Terran 1".to_string(), "light industry".to_string()),
        // Action::Wait(10),
        // Action::AddFacility("Terran 1".to_string(), "heavy industry".to_string()),
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