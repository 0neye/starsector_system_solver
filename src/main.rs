mod constants;
mod utils;
mod planet;
mod system;
mod solver;
mod parser;

use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    println!("Loading game data...");
    
    let systems = parser::load_game_data(
        "Planets.csv",
        "Infrastructure.csv"
    )?;
    
    println!("\nLoaded {} systems:", systems.len());
    
    for (system_name, system) in &systems {
        println!("\nSystem: {}", system_name);
        
        // Print planets
        println!("  Planets:");
        for (planet_name, planet) in system.planets() {
            println!("    - {} (Hazard: {}%, Size: {})", 
                planet_name,
                planet.hazard_rating(),
                planet.size()
            );
        }
        
        // Print infrastructure
        println!("  Infrastructure:");
        for (infra_name, _infra) in system.infrastructure() {
            println!("    - {}", infra_name);
        }
        
        // Print stability bonus from comm relay
        let has_relay = system.has_comm_relay();
        println!("  Has Comm Relay: {} (Stability Bonus: {})",
            has_relay,
            if has_relay { "+2" } else { "0" }
        );
    }
    
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

- Add score function to system/state structs
- Start on the minimax/alpha-beta with progressive deepening algorithm
- Add constraints beyond the balance struct, so that it has to build defensive structures
- Implement system-wide restrictions on commerce facility
*/