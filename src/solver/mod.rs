pub mod bound;
pub mod cancel;
pub mod decomp;
pub mod goal;
pub mod pareto;
pub mod state;

pub use bound::{credits_relaxed, BoundRow};
pub use decomp::stats as decomp_stats;
pub use decomp::{
    diagnose_maximize_gap, search_system_decomp, search_system_decomp_with_settings,
    search_system_maximize, search_system_maximize_with_settings,
};
pub use goal::{AStarSearchResult, Goal, Metric};
pub use pareto::{
    solve_pareto, solve_pareto_bound, solve_pareto_bound_with_settings, solve_pareto_quick,
    solve_pareto_quick_with_settings, solve_pareto_template, solve_pareto_template_with_settings,
    solve_pareto_with_settings,
};
pub(crate) use state::improvement_story_point_cost;
pub use state::{Action, Balance, State};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SolverSettings {
    /// Include story-point improvements and alpha-core installs on industries
    /// and structures.
    pub include_industry_upgrades: bool,
    /// Modded behavior: allow multiple industries/structures to build on the
    /// same colony at the same time. Vanilla Starsector queues them one at a
    /// time per colony.
    pub allow_parallel_builds: bool,
}

impl Default for SolverSettings {
    fn default() -> Self {
        Self {
            include_industry_upgrades: true,
            allow_parallel_builds: false,
        }
    }
}

impl SolverSettings {
    /// Settings that preserve the pre-`SolverSettings` library behavior:
    /// builds were never queue-constrained (`allow_parallel_builds: true`).
    /// Used by the backward-compatible `bool include_industry_upgrades`
    /// wrappers so each call site doesn't repeat the literal.
    pub fn legacy(include_industry_upgrades: bool) -> Self {
        Self {
            include_industry_upgrades,
            allow_parallel_builds: true,
        }
    }

    pub fn exclude_upgrades(self) -> bool {
        !self.include_industry_upgrades
    }
}

/// Whether two states' planets disagree on which facilities exist. Used by the
/// apply/undo consistency checker below.
fn _fac_inconsistency(state1: &State, state2: &State) -> bool {
    for planet_name in state1.system().planets().keys() {
        let facilities1 = &state1.system().planets()[planet_name].facilities();
        let facilities2 = &state2.system().planets()[planet_name].facilities();

        if facilities1.len() != facilities2.len() {
            return true;
        }

        if !facilities1
            .iter()
            .zip(facilities2.iter())
            .all(|(fac1, fac2)| fac1.name() == fac2.name())
        {
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
                println!(
                    "Should be: {:#?}",
                    should_be
                        .system()
                        .planets()
                        .get(planet_name)
                        .unwrap()
                        .facilities()
                        .iter()
                        .map(|f| f.name())
                        .collect::<Vec<_>>()
                );
                println!(
                    "Is: {:#?}",
                    is.system()
                        .planets()
                        .get(planet_name)
                        .unwrap()
                        .facilities()
                        .iter()
                        .map(|f| f.name())
                        .collect::<Vec<_>>()
                );
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
            println!(
                "\nBlank State - Credits: {}",
                blank_state.balance().credits()
            );
            println!(
                "Blank State - Facilities: {:?}",
                blank_state
                    .system()
                    .planets()
                    .values()
                    .flat_map(|p| p
                        .facilities()
                        .iter()
                        .map(|f| (f.name(), f.remaining_build_days())))
                    .collect::<Vec<_>>()
            );
            println!(
                "Should be State - Credits: {}",
                should_be.balance().credits()
            );
            println!(
                "Should be State - Facilities: {:?}",
                should_be
                    .system()
                    .planets()
                    .values()
                    .flat_map(|p| p
                        .facilities()
                        .iter()
                        .map(|f| (f.name(), f.remaining_build_days())))
                    .collect::<Vec<_>>()
            );
            println!("Is State - Credits: {}", is.balance().credits());
            println!(
                "Is State - Facilities: {:?}",
                is.system()
                    .planets()
                    .values()
                    .flat_map(|p| p
                        .facilities()
                        .iter()
                        .map(|f| (f.name(), f.remaining_build_days())))
                    .collect::<Vec<_>>()
            );
            println!("Action log: {:?}", actions);
            panic!();
        }
        temp_state.undo_last_action(false);
    }
}
