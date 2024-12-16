mod constants;
mod utils;
mod planet;
mod system;
mod solver;
mod parser;

use std::error::Error;
use std::collections::HashMap;
use solver::{simulate_linear, search, State, Balance, SearchInfo};
use constants::ColonyItem;
use solver::Action;
use system::System;

fn test_solver(state: State) {
    println!("\nStarting solver test...");
    println!("Initial state score: {}", state.score() as i32);
    
    // Run simulation for 10 turns
    // simulate_linear(&state, 15);

    println!("{:#?}", search(&state, 20000).action_log);
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
        10_000_000.0,  // 1M credits
        5,             // 5 story points
        5,             // 5 alpha cores
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
    
    // Run solver test
    test_solver(state);

    // // Test growth update
    // // Apply actions to set up the initial state
    // state.apply_action_raw(&Action::Colonize("Terran 1".to_string()), true);
    // state.apply_action_raw(&Action::AddFacility("Terran 1".to_string(), "mining".to_string()), true);
    // // state.apply_action_raw(&Action::AddFacility("Terran 1".to_string(), "waystation".to_string()), true);
    // state.apply_action_raw(&Action::Wait(35), true);
    // // state.apply_action_raw(&&Action::SetFreePort("Terran 1".to_string(), true), true);
    // state.apply_action_raw(&Action::AddFacility("Terran 1".to_string(), "farming".to_string()), true);
    // state.apply_action_raw(&Action::Wait(35), true);
    // state.apply_action_raw(&Action::AddFacility("Terran 1".to_string(), "heavy industry".to_string()), true);
    // state.apply_action_raw(&Action::Wait(35), true);
    // state.apply_action_raw(&Action::AddFacility("Terran 1".to_string(), "refining".to_string()), true);


    // // state.apply_action_raw(&Action::SetFreePort("Terran 1".to_string(), true), true);
    // // state.apply_action_raw(&Action::SetHazardPay("Barren 1".to_string(), true), true);


    // println!("\nInitial state:");
    // println!("{:#?}", state.balance());

    // let initial_credits = state.balance().credits();

    // // Apply test action
    // state.apply_action_raw(&Action::AddFacility("Terran 1".to_string(), "orbital station".to_string()), true);
    // println!("\nState after applying test action:");
    // println!("{:#?}", state.system().planets().get("Terran 1").unwrap().facilities().keys().collect::<Vec<_>>());
    // println!("Possible actions after test action:");
    // println!("{:#?}", state.get_possible_actions(true));

    // // Calculate and print the difference in credits
    // let final_credits = state.balance().credits();
    // let credit_difference = final_credits - initial_credits;
    // println!("\nCredit change from test action: {}", credit_difference);

    // // Undo the wait action
    // state.undo_last_action(true);
    // println!("\nState after undoing the test action:");
    // println!("{:#?}", state.system().planets().get("Terran 1").unwrap().facilities().keys().collect::<Vec<_>>());

    // // Verify that credits are back to the initial value
    // assert_eq!(state.balance().credits(), initial_credits, "Credits should be back to the initial value after undo");
    // println!("Credits successfully reverted to initial value: {}", initial_credits);




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
*/