pub mod state;
pub mod goal;
pub mod decomp;
pub mod pareto;
pub mod bound;
pub mod archive;

pub use state::{State, Balance, Action};
pub use goal::{AStarSearchResult, Goal, Metric};
pub use decomp::{diagnose_maximize_gap, search_system_decomp, search_system_maximize};
pub use pareto::solve_pareto;
pub use bound::{credits_relaxed, BoundRow};

/// Whether two states' planets disagree on which facilities exist. Used by the
/// apply/undo consistency checker below.
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

/// Replaying an action log from scratch must reproduce exactly the same
/// facilities and credits as undoing it step by step. This is the core guarantee
/// that lets a search undo its way back up the tree; panics on any drift.
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
