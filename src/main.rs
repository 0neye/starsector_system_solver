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

fn test_solver(state: State) {
    println!("\nStarting solver test...");
    println!("Initial state score: {}", state.score() as i32);
    
    // Run simulation for 10 turns
    simulate_linear(&state, 15);

    // println!("{:#?}", search(&state, 5000).action_log);
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("Loading game data...");
    
    let systems = parser::load_game_data(
        "Planets.csv",
        "Infrastructure.csv"
    )?;
    
    // Create a test case with a single system
    let test_system = systems.get("Mia Bravos").unwrap().clone();
    
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

    // Test growth update
    // state.apply_action_raw(&Action::Colonize("Terran 1".to_string()));
    // println!("\nCurrent balance after colonization:");
    // println!("{:#?}", state.balance());

    // let days = state.system().get_planet("Terran 1").unwrap().days_till_next_size(None, None).unwrap() + 10;
    
    // state.apply_action_raw(&Action::Wait((days as f32 / 30.0).ceil() as u32));
    // // println!("\nPlanet 'Terran 1' after waiting {} days:", days);
    // // println!("{:#?}", state.system().get_planet("Terran 1").unwrap());
    // println!("\nCurrent balance after wait:");
    // println!("{:#?}", state.balance());

    // state.undo_last_action();
    // println!("\nCurrent balance after undo:");
    // println!("{:#?}", state.balance());


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
- Add constraints beyond the balance struct, so that it has to build defensive structures - Done
- Hash action sequences instead of system state - Done
- Add the ability to undo any action so we can efficiently search the game tree - Done

- Get a DFBB search working
- Implement system-wide restrictions on commerce facility
- Redo the upkeep/production formulas to either be a struct or function pointer so we don't have to parse them
*/