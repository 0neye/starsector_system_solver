//! Archived IDA* solver (superseded by the joint decomposition solver in
//! [`crate::solver::decomp`]). Kept for comparison/benchmarking; not on the
//! default path. The `Goal` and `AStarSearchResult` types it uses now live in
//! [`crate::solver::goal`]; the IDA*-specific admissible-bound methods are
//! attached here as a separate `impl Goal` block.

use crate::constants::{AdminType, FacilityType, FACILITY_DATA};
use crate::planet::Facility;
use crate::solver::goal::{AStarSearchResult, Goal};
use crate::solver::state::{Action, State};
use crate::system::System;
use nohash_hasher::{BuildNoHashHasher, NoHashHasher};
use rayon::prelude::*;
use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::time::{Duration, Instant};

fn days_to_months_ceil(days: u32) -> i32 {
    ((days + 29) / 30) as i32
}

impl Goal {
    /// Returns an admissible lower-bound on the number of months
    /// needed for `state` to satisfy the goal, using the precomputed
    /// facility data in `precompute_map`.
    ///
    /// 1) Check how close we already are to each goal metric.
    /// 2) If shortfall > 0, gather candidate facilities from *unbuilt* ones in `state`.
    /// 3) Estimate how many months to cover each shortfall in parallel.
    /// 4) Final heuristic is the maximum of those times (for an admissible approach).
    pub fn month_lower_bound(
        &self,
        state: &State,
        precompute_map: &HashMap<
            u64,
            HashMap<FacilityType, PrecomputedFacilityData, BuildNoHashHasher<u8>>,
            BuildNoHashHasher<u64>,
        >,
    ) -> i32 {
        // Gather all unbuilt facilities once
        let mut unbuilt_map = HashMap::with_hasher(BuildNoHashHasher::default());
        for (planet_hash, planet) in state.system().planets() {
            unbuilt_map.insert(*planet_hash, planet.unbuilt_facilities(false).to_vec());
        }

        let current_net_income = state.balance().net_income();
        let net_income_shortfall = (self.min_net_income - current_net_income).max(0.0);

        let current_def = state.system().avg_ground_defense();
        let defense_shortfall = if let Some(req_def) = self.min_ground_defense {
            (req_def - current_def).max(0.0)
        } else {
            0.0
        };

        let current_stab = state.system().avg_stability();
        let stab_shortfall = if let Some(req_stab) = self.min_stability {
            (req_stab as f64 - current_stab).max(0.0)
        } else {
            0.0
        };

        let ni_months = self.estimate_months_for_income(
            net_income_shortfall,
            state,
            precompute_map,
            &unbuilt_map,
        );
        let def_months = self.estimate_months_for_defense(
            defense_shortfall,
            state,
            precompute_map,
            &unbuilt_map,
        );
        let stab_months =
            self.estimate_months_for_stability(stab_shortfall, state, precompute_map, &unbuilt_map);

        ni_months.max(def_months).max(stab_months)
    }

    /// Estimate months to cover the `income_shortfall`, based on
    /// facilities that are currently *unbuilt* in `state`.
    pub fn estimate_months_for_income(
        &self,
        income_shortfall: f64,
        state: &State,
        precompute_map: &HashMap<
            u64,
            HashMap<FacilityType, PrecomputedFacilityData, BuildNoHashHasher<u8>>,
            BuildNoHashHasher<u64>,
        >,
        unbuilt_map: &HashMap<u64, Vec<FacilityType>, BuildNoHashHasher<u64>>,
    ) -> i32 {
        if income_shortfall <= 0.0 {
            return 0;
        }

        // Optimistic lower bound: assume all eligible facilities can be started now in parallel.
        // Find the earliest completion month `t` such that the sum of incomes of facilities with
        // build_time <= t covers the shortfall.
        let candidates: Vec<&PrecomputedFacilityData> = gather_facility_candidates_by_metric(
            state.system(),
            precompute_map,
            |data| data.net_income > 0.0, // We only care about facilities that help net income
            unbuilt_map,
        );

        let mut candidates_by_time: Vec<(i32, f64)> = candidates
            .into_iter()
            .map(|c| (days_to_months_ceil(c.build_time), c.net_income))
            .collect();

        candidates_by_time.sort_by(|a, b| a.0.cmp(&b.0));

        let mut accumulated = 0.0;
        for (months, income) in candidates_by_time {
            accumulated += income;
            if accumulated + 1e-9 >= income_shortfall {
                return months;
            }
        }

        // If facility-only estimates can't cover the gap (common when growth drives income),
        // fall back to an optimistic bound instead of "infinite" so IDA* can still progress.
        0
    }

    /// Estimate months to cover the `defense_shortfall`, based on
    /// facilities that are currently *unbuilt* in `state`.
    pub fn estimate_months_for_defense(
        &self,
        defense_shortfall: f64,
        state: &State,
        precompute_map: &HashMap<
            u64,
            HashMap<FacilityType, PrecomputedFacilityData, BuildNoHashHasher<u8>>,
            BuildNoHashHasher<u64>,
        >,
        unbuilt_map: &HashMap<u64, Vec<FacilityType>, BuildNoHashHasher<u64>>,
    ) -> i32 {
        if defense_shortfall <= 0.0 {
            return 0;
        }

        // Optimistic lower bound similar to income/stability.
        // We intentionally over-estimate each facility's defense contribution to avoid ever
        // over-estimating the required months (admissibility).
        //
        // Planet::ground_defense_strength() base term peaks at size 6 with stability 10:
        // base = 100*(6-3)=300 and stability factor = 0.25 + 10*0.075 = 1.0.
        const MAX_BASE_DEFENSE_STRENGTH: f64 = 300.0;

        let num_planets = state.system().planets().len() as f64;
        let total_shortfall = defense_shortfall * num_planets;

        let mut candidates_by_time: Vec<(i32, f64)> = Vec::new();
        for (planet_hash, facility_map) in precompute_map {
            if let Some(unbuilt) = unbuilt_map.get(planet_hash) {
                for facility_type in unbuilt {
                    if let Some(fac_data) = facility_map.get(facility_type) {
                        if fac_data.defense_mult > 1.0 {
                            let delta = MAX_BASE_DEFENSE_STRENGTH * (fac_data.defense_mult - 1.0);
                            if delta > 0.0 {
                                candidates_by_time
                                    .push((days_to_months_ceil(fac_data.build_time), delta));
                            }
                        }
                    }
                }
            }
        }

        candidates_by_time.sort_by(|a, b| a.0.cmp(&b.0));

        let mut accumulated = 0.0;
        for (months, delta) in candidates_by_time {
            accumulated += delta;
            if accumulated + 1e-9 >= total_shortfall {
                return months;
            }
        }

        0
    }

    /// Estimate months to cover the `stability_shortfall`, based on
    /// facilities that are currently *unbuilt* in `state`.
    pub fn estimate_months_for_stability(
        &self,
        stability_shortfall: f64,
        state: &State,
        precompute_map: &HashMap<
            u64,
            HashMap<FacilityType, PrecomputedFacilityData, BuildNoHashHasher<u8>>,
            BuildNoHashHasher<u64>,
        >,
        unbuilt_map: &HashMap<u64, Vec<FacilityType>, BuildNoHashHasher<u64>>,
    ) -> i32 {
        if stability_shortfall <= 0.0 {
            return 0;
        }

        // Optimistic lower bound: assume all eligible facilities can be started now in parallel.
        // Find earliest completion month where summed stability bonuses cover the shortfall.
        let candidates: Vec<&PrecomputedFacilityData> = gather_facility_candidates_by_metric(
            state.system(),
            precompute_map,
            |data| data.stability_bonus > 0,
            unbuilt_map,
        );

        let mut candidates_by_time: Vec<(i32, f64)> = candidates
            .into_iter()
            .map(|c| (days_to_months_ceil(c.build_time), c.stability_bonus as f64))
            .collect();

        candidates_by_time.sort_by(|a, b| a.0.cmp(&b.0));

        let mut accumulated = 0.0;
        for (months, bonus) in candidates_by_time {
            accumulated += bonus;
            if accumulated + 1e-9 >= stability_shortfall {
                return months;
            }
        }

        0
    }
}

/// Utility function that returns a flat list of references to
/// `PrecomputedFacilityData` for all currently unbuilt facilities on all planets
/// that pass a certain predicate (e.g., net_income > 0).
///
/// 1. For each planet in the system, retrieve unbuilt facility types via `unbuilt_facilities`.  
/// 2. For each unbuilt facility type, look up the corresponding `PrecomputedFacilityData`
///    in `precompute_map`, if it exists.  
/// 3. Filter by `predicate` to restrict to those that help for the metric in question.  
/// 4. Collect them into a flat Vec.
fn gather_facility_candidates_by_metric<'a, F>(
    system: &System,
    precompute_map: &'a HashMap<
        u64,
        HashMap<FacilityType, PrecomputedFacilityData, BuildNoHashHasher<u8>>,
        BuildNoHashHasher<u64>,
    >,
    predicate: F,
    unbuilt_map: &HashMap<u64, Vec<FacilityType>, BuildNoHashHasher<u64>>,
) -> Vec<&'a PrecomputedFacilityData>
where
    F: Fn(&PrecomputedFacilityData) -> bool,
{
    let mut result = Vec::new();

    for planet_hash in system.planets().keys() {
        // Safely get unbuilt list - skip if not found
        let Some(unbuilt_list) = unbuilt_map.get(planet_hash) else {
            continue;
        };

        if let Some(facility_map) = precompute_map.get(planet_hash) {
            for fac_type in unbuilt_list {
                if let Some(precomp_data) = facility_map.get(&fac_type) {
                    if predicate(precomp_data) {
                        result.push(precomp_data);
                    }
                }
            }
        }
    }

    result
}

// A record of how valuable a facility might be on a particular planet
#[derive(Debug, Clone)]
pub struct PrecomputedFacilityData {
    pub facility_type: FacilityType,
    pub net_income: f64,
    pub stability_bonus: i32,
    pub defense_mult: f64,
    pub build_time: u32,
}

/// Returns a nested map:  planet_hash -> (facility_type -> PrecomputedFacilityData),
/// which can be used to quickly look up the "best" options among all unbuilt
/// facilities for each planet. <br> We ignore facility-based `income_multiplier`
/// (as requested), but *do* assume alpha cores/items/improvements that
/// contribute stability, defense, or flat income, etc.
pub fn precompute_facility_candidates(
    state: &State,
) -> HashMap<
    u64,
    HashMap<FacilityType, PrecomputedFacilityData, BuildNoHashHasher<u8>>,
    BuildNoHashHasher<u64>,
> {
    let mut result: HashMap<
        u64,
        HashMap<FacilityType, PrecomputedFacilityData, BuildNoHashHasher<u8>>,
        BuildNoHashHasher<u64>,
    > = HashMap::with_hasher(BuildNoHashHasher::default());
    let mut system = state.system().clone();

    // Optimistic: assume we can achieve the best available income multiplier eventually.
    let max_income_mult = FACILITY_DATA
        .values()
        .fold(1.0_f64, |acc, data| acc.max(data.income_multiplier));

    // For each planet in the system
    for (planet_hash, planet) in system.planets_mut() {
        // Heuristic precompute assumes we can colonize if needed (optimistic lower bound).
        if !planet.has_colony() {
            planet.set_has_colony(true);
        }

        // Apply all bonuses
        planet.set_admin(AdminType::AlphaCore);
        planet.set_free_port(true);
        _ = planet.wait(200, false); // maybe change this to be more efficient, or save values based on planet size

        // If the planet can build new facilities
        let unbuilt_fac_types = planet.unbuilt_facilities(false);

        let mut facility_map: HashMap<
            FacilityType,
            PrecomputedFacilityData,
            BuildNoHashHasher<u8>,
        > = HashMap::with_hasher(BuildNoHashHasher::default());

        // For each unbuilt facility type
        for fac_type in unbuilt_fac_types {
            let mut facility = match Facility::new(fac_type) {
                Some(fac) => fac,
                None => continue, // skip if something invalid
            };

            // Retrieve the facility’s base data
            let facility_data_opt = facility.get_data();
            if facility_data_opt.is_none() {
                // skip if no data
                continue;
            }
            let facility_data = facility_data_opt.unwrap().clone();

            let build_time = facility_data.build_time;

            // Apply all bonuses
            facility.add_improvements();
            facility.add_alpha_core();
            let items = facility.get_possible_colony_items(planet);
            if let Some(item) = items.first() {
                facility.add_colony_item(*item, planet);
            }

            facility.progress_build_days(facility_data.build_time as i32 + 1);

            // From megaport, fullerene spool, improvements, alpha core
            let max_access = planet.calculate_accessibility() + 20.0 + 30.0 + 20.0 + 20.0;

            let fac_gross_income =
                facility.calculate_gross_income(planet.size(), planet, max_access);
            let fac_upkeep = facility.calculate_upkeep(planet.hazard_rating(), planet.size());
            let fac_net_income = fac_gross_income * max_income_mult - fac_upkeep;

            let fac_stability_bonus = facility.calculate_stability_bonus();

            let fac_defense_mult = facility.calculate_defense_multiplier();

            // Finally, store the precomputed data for this planet + facility type
            facility_map.insert(
                fac_type,
                PrecomputedFacilityData {
                    facility_type: fac_type,
                    net_income: fac_net_income,
                    stability_bonus: fac_stability_bonus,
                    defense_mult: fac_defense_mult,
                    build_time,
                },
            );
        }

        // Put the facility_map for this planet into the main result
        result.insert(*planet_hash, facility_map);
    }

    // Done
    result
}

fn action_cost(action: &Action) -> i32 {
    match action {
        Action::Wait(months) => *months as i32,
        _ => 0,
    }
}

fn ida_star(
    initial_state: &mut State,
    goal: &Goal,
    time_limit: u32,
    exclude_upgrades: bool,
) -> Option<AStarSearchResult> {
    println!("Starting IDA* search with time limit: {} ms", time_limit);
    let start_time = Instant::now();
    let precompute_map = precompute_facility_candidates(initial_state);
    let mut bound = goal.month_lower_bound(initial_state, &precompute_map);
    let mut visited: HashMap<u64, i32, BuildNoHashHasher<u64>> =
        HashMap::with_capacity_and_hasher(1000000, BuildNoHashHasher::default());
    let mut total_nodes_searched = 0;
    let mut total_nodes_pruned = 0;
    let debug = std::env::var_os("SYSTEM_SOLVER_IDASTAR_DEBUG").is_some();

    loop {
        let root_hash_before = if debug {
            Some(initial_state.get_deep_hash())
        } else {
            None
        };
        let root_credits_before = if debug {
            Some(initial_state.balance().credits())
        } else {
            None
        };

        println!("Current bound: {:?}", bound);
        let iteration_start = Instant::now();
        visited.clear();
        visited.insert(initial_state.get_deep_hash(), 0);

        if debug {
            let actions_len = initial_state.get_possible_actions(exclude_upgrades).len();
            let colonized = initial_state
                .system()
                .planets()
                .values()
                .filter(|p| p.has_colony())
                .count();
            println!(
                "[ida*] root credits: {:.0}, planets: {}, colonized: {}, actions: {}",
                initial_state.balance().credits(),
                initial_state.system().planets().len(),
                colonized,
                actions_len
            );
        }

        let result = depth_limited_search(
            initial_state,
            goal,
            0,
            bound,
            &mut visited,
            &precompute_map,
            exclude_upgrades,
        );
        total_nodes_searched += result.nodes_searched;
        total_nodes_pruned += result.nodes_pruned_by_bound;

        if debug {
            let hash_after = initial_state.get_deep_hash();
            let credits_after = initial_state.balance().credits();
            if root_hash_before.is_some_and(|h| h != hash_after)
                || root_credits_before.is_some_and(|c| (c - credits_after).abs() > 0.5)
            {
                println!(
                    "[ida*] WARNING: root state mutated across iteration: hash {} -> {}, credits {:.0} -> {:.0}",
                    root_hash_before.unwrap_or(0),
                    hash_after,
                    root_credits_before.unwrap_or(0.0),
                    credits_after
                );
            }
        }

        let elapsed = iteration_start.elapsed();
        if elapsed >= Duration::from_secs(1) {
            let nodes_per_sec = visited.len() as f64 / elapsed.as_secs_f64();
            println!("Unique nodes/sec: {:.2}", nodes_per_sec);
        }

        if result.solution.is_some() {
            println!("Solution found!");
            println!("Total nodes searched: {}", total_nodes_searched);
            println!("Total nodes pruned by bound: {}", total_nodes_pruned);
            println!("Total unique positions searched: {}", visited.len());
            return Some(AStarSearchResult {
                solution: result.solution,
                cost: result.cost,
                cutoff_occurred: false,
                nodes_searched: total_nodes_searched,
                nodes_pruned_by_bound: total_nodes_pruned,
            });
        }

        if result.cutoff_occurred {
            println!("Cutoff occurred. Increasing bound to {:?}", result.cost);
            println!(
                "Nodes searched in this iteration: {}",
                result.nodes_searched
            );
            println!(
                "Nodes pruned by bound in this iteration: {}",
                result.nodes_pruned_by_bound
            );
            println!("Unique positions in this iteration: {}", visited.len());
            bound = result.cost;
        } else {
            println!("No solution found within current bound");
            println!("Total nodes searched: {}", total_nodes_searched);
            println!("Total nodes pruned by bound: {}", total_nodes_pruned);
            println!("Total unique positions searched: {}", visited.len());
            return None;
        }

        if start_time.elapsed() > Duration::from_millis(time_limit as u64) {
            println!("Time limit of {} ms exceeded", time_limit);
            println!("Total nodes searched: {}", total_nodes_searched);
            println!("Total nodes pruned by bound: {}", total_nodes_pruned);
            println!("Total unique positions searched: {}", visited.len());
            return None;
        }
    }
}

fn depth_limited_search(
    state: &mut State,
    goal: &Goal,
    g: i32,
    bound: i32,
    visited: &mut HashMap<u64, i32, BuildHasherDefault<NoHashHasher<u64>>>,
    precompute_map: &HashMap<
        u64,
        HashMap<FacilityType, PrecomputedFacilityData, BuildNoHashHasher<u8>>,
        BuildNoHashHasher<u64>,
    >,
    exclude_upgrades: bool,
) -> AStarSearchResult {
    if goal.is_satisfied(state) {
        return AStarSearchResult {
            solution: Some(state.action_log().clone()),
            cost: g,
            cutoff_occurred: false,
            nodes_searched: 1,
            nodes_pruned_by_bound: 0,
        };
    }

    let h = goal.month_lower_bound(state, precompute_map);
    let f = g.saturating_add(h);

    // Prune by bound
    if f > bound {
        return AStarSearchResult {
            solution: None,
            cost: f,
            cutoff_occurred: true,
            nodes_searched: 1,
            nodes_pruned_by_bound: 1,
        };
    }

    let mut nodes_searched = 1; // current node
    let mut nodes_pruned_by_bound = 0;
    let mut cutoff_occurred = false;
    let mut best_next_bound = i32::MAX;

    for action in state.get_ordered_possible_actions(exclude_upgrades) {
        let cost = action_cost(&action);
        state.apply_action_raw(&action, false);
        let next_hash = state.get_deep_hash();
        let next_g = g + cost;
        if let Some(best_g) = visited.get(&next_hash) {
            if next_g >= *best_g {
                state.undo_last_action(false);
                nodes_searched += 1;
                continue;
            }
        }
        visited.insert(next_hash, next_g);

        let result = depth_limited_search(
            state,
            goal,
            next_g,
            bound,
            visited,
            precompute_map,
            exclude_upgrades,
        );
        state.undo_last_action(false);

        if result.solution.is_some() {
            return AStarSearchResult {
                solution: result.solution,
                cost: result.cost,
                cutoff_occurred: false,
                nodes_searched: nodes_searched + result.nodes_searched,
                nodes_pruned_by_bound: nodes_pruned_by_bound + result.nodes_pruned_by_bound,
            };
        }

        nodes_searched += result.nodes_searched;
        nodes_pruned_by_bound += result.nodes_pruned_by_bound;

        if result.cutoff_occurred {
            cutoff_occurred = true;
        }
        best_next_bound = best_next_bound.min(result.cost);
    }

    AStarSearchResult {
        solution: None,
        cost: best_next_bound,
        cutoff_occurred,
        nodes_searched,
        nodes_pruned_by_bound,
    }
}

pub fn search_all_planets(
    initial_state: &mut State,
    goal: &Goal,
    time_limit: u32,
    exclude_upgrades: bool,
) -> Vec<AStarSearchResult> {
    // Split state into per-planet states
    let planet_states = initial_state.to_vec_by_planet();

    // Search each planet in parallel
    planet_states
        .into_par_iter()
        .filter_map(|mut state| ida_star(&mut state, goal, time_limit, exclude_upgrades))
        .collect()
}

/*
TODOS:
- Precalculate the maximum gross income, defense, and stability bonuses for each facility
- Use that with the state in the heuristic function to calculate the minimum possible months to wait
    to achieve the goal
- Maybe still cost non-wait actions based on their construction time?
*/
