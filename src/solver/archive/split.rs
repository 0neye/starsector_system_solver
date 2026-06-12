//! Archived per-planet decomposition entry point.
//!
//! Decomposes the system into independent per-planet sub-problems and solves
//! each in parallel with [`crate::solver::decomp::decomp_search`]. Superseded by
//! the joint solver ([`crate::solver::decomp::search_system_decomp`]): this split
//! copies the full budget into every planet and shares neither income nor
//! one-off resources across them, and it forces each planet to meet the goal
//! alone rather than the system-wide total. Kept for comparison.

use rayon::prelude::*;

use crate::solver::decomp::decomp_search;
use crate::solver::goal::{AStarSearchResult, Goal};
use crate::solver::state::State;

pub fn search_all_planets_decomp(
    initial_state: &mut State,
    goal: &Goal,
    time_limit: u32,
    exclude_upgrades: bool,
) -> Vec<AStarSearchResult> {
    let planet_states = initial_state.to_vec_by_planet();

    planet_states
        .into_par_iter()
        .filter_map(|mut state| decomp_search(&mut state, goal, time_limit, exclude_upgrades))
        .collect()
}
