//! Two-level decomposition solver.
//!
//! The IDA* solver in [`crate::solver::astar`] minimizes total months waited,
//! but its goal is a threshold in *output* space (net income / defense /
//! stability) while its only cost is *time*. That mismatch makes the heuristic
//! weak, and because every build/colonize/toggle action is zero-cost, IDA*
//! re-explores huge plateaus of action permutations each iteration.
//!
//! This module attacks the problem the way it actually factors:
//!
//! * **Level 1 (outer, [`decomp_search`])** chooses a *system plan* — per planet,
//!   which facilities, upgrades, items and toggles to include — and searches
//!   over plans to minimize the months reported by Level 2.
//! * **Level 2 (inner, [`simulate_plan`])** treats a fixed plan as a nearly
//!   forced schedule: apply every plan action that is legal+affordable right
//!   now, and when nothing is immediately doable, `Wait` the smallest
//!   meaningful interval. Because feasibility is monotonic in time for a fixed
//!   plan, "build ASAP, wait to the next event" minimizes makespan for it.
//!
//! Because the plan is keyed per planet and the simulation runs on the whole
//! `State`, [`decomp_search`] handles a *multi-planet system on one shared
//! timeline*: a single `Wait` advances every planet, and credits/cores/story
//! points/items are spent from the common pool — so it optimizes the true
//! system-wide goal with correctly shared resources.
//!
//! [`search_system_decomp`] is the entry point: one combined plan over the whole
//! system on a shared timeline and budget. The older per-planet split lives in
//! [`crate::solver::archive::split`] for comparison.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::constants::{FacilityType, FACILITY_DATA};
use crate::solver::goal::{AStarSearchResult, Goal};
use crate::solver::state::{Action, State};

/// Credits required to colonize a planet (mirrors `apply_action_raw`).
const COLONIZE_COST: f64 = 125_000.0;
/// Hard ceiling on simulated months so an infeasible plan bails instead of
/// looping forever (100 years).
const MAX_PLAN_MONTHS: i32 = 1_200;
/// Guard against a plan that keeps acting/waiting without ever satisfying the
/// goal (e.g. income asymptotes below the target).
const MAX_SIM_ITERS: u32 = 5_000;

#[inline]
fn is_wait(action: &Action) -> bool {
    matches!(action, Action::Wait(_))
}

/// The structural decisions to include for a single planet. The ordering and
/// timing of the resulting actions is left to Level 2.
#[derive(Clone, Debug, Default)]
struct PlanetPlan {
    colonize: bool,
    free_port: bool,
    hazard_pay: bool,
    upgrade_admin: bool,
    improvements: bool,
    alpha_cores: bool,
    install_items: bool,
    facilities: HashSet<FacilityType>,
}

impl PlanetPlan {
    /// Number of distinct decisions; used to break ties toward simpler plans.
    fn size(&self) -> usize {
        self.facilities.len()
            + [
                self.colonize,
                self.free_port,
                self.hazard_pay,
                self.upgrade_admin,
                self.improvements,
                self.alpha_cores,
                self.install_items,
            ]
            .iter()
            .filter(|b| **b)
            .count()
    }
}

/// A Level-1 candidate over the whole system: one [`PlanetPlan`] per planet.
///
/// `pub(crate)` so the in-crate test suite can construct fixed plans and drive
/// the Level-2 simulator directly.
#[derive(Clone, Debug, Default)]
pub(crate) struct SystemPlan {
    planets: HashMap<u64, PlanetPlan>,
}

impl SystemPlan {
    /// Whether this plan permits taking `action`. Legality/affordability is
    /// already enforced by the action generator; this only encodes plan intent.
    /// The planet is identified by the hash every non-`Wait` action carries.
    fn allows(&self, action: &Action) -> bool {
        match action {
            Action::AddFacility(h, ft) => {
                self.planets.get(h).is_some_and(|p| p.facilities.contains(ft))
            }
            Action::AddImprovement(h, _) => self.planets.get(h).is_some_and(|p| p.improvements),
            Action::AddAlphaCore(h, _) => self.planets.get(h).is_some_and(|p| p.alpha_cores),
            Action::InstallItem(h, _, _) => self.planets.get(h).is_some_and(|p| p.install_items),
            Action::SetFreePort(h, true) => self.planets.get(h).is_some_and(|p| p.free_port),
            Action::SetHazardPay(h, true) => self.planets.get(h).is_some_and(|p| p.hazard_pay),
            Action::UpgradeAdmin(h) => self.planets.get(h).is_some_and(|p| p.upgrade_admin),
            Action::Colonize(h) => self.planets.get(h).is_some_and(|p| p.colonize),
            // The plan never deliberately disables a bonus.
            Action::SetFreePort(_, false) | Action::SetHazardPay(_, false) => false,
            Action::Wait(_) => false,
        }
    }

    /// Total number of decisions across all planets.
    fn size(&self) -> usize {
        self.planets.values().map(PlanetPlan::size).sum()
    }

    /// A plan that permits every action on every planet (all facilities, all
    /// toggles). Used by the test suite to drive the Level-2 simulator like an
    /// unconstrained greedy build; the generator still enforces legality.
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn permit_all(state: &State) -> Self {
        let mut planets = HashMap::new();
        for hash in state.system().planets().keys() {
            planets.insert(
                *hash,
                PlanetPlan {
                    colonize: true,
                    free_port: true,
                    hazard_pay: true,
                    upgrade_admin: true,
                    improvements: true,
                    alpha_cores: true,
                    install_items: true,
                    facilities: FACILITY_DATA.keys().copied().collect(),
                },
            );
        }
        SystemPlan { planets }
    }
}

/// The toggleable (non-facility) per-planet decisions, so the hill-climb can
/// iterate them.
#[derive(Clone, Copy)]
enum Toggle {
    Colonize,
    FreePort,
    HazardPay,
    UpgradeAdmin,
    Improvements,
    AlphaCores,
    InstallItems,
}

impl Toggle {
    const ALL: [Toggle; 7] = [
        Toggle::Colonize,
        Toggle::FreePort,
        Toggle::HazardPay,
        Toggle::UpgradeAdmin,
        Toggle::Improvements,
        Toggle::AlphaCores,
        Toggle::InstallItems,
    ];

    fn get(self, plan: &PlanetPlan) -> bool {
        match self {
            Toggle::Colonize => plan.colonize,
            Toggle::FreePort => plan.free_port,
            Toggle::HazardPay => plan.hazard_pay,
            Toggle::UpgradeAdmin => plan.upgrade_admin,
            Toggle::Improvements => plan.improvements,
            Toggle::AlphaCores => plan.alpha_cores,
            Toggle::InstallItems => plan.install_items,
        }
    }

    fn set(self, plan: &mut PlanetPlan, value: bool) {
        match self {
            Toggle::Colonize => plan.colonize = value,
            Toggle::FreePort => plan.free_port = value,
            Toggle::HazardPay => plan.hazard_pay = value,
            Toggle::UpgradeAdmin => plan.upgrade_admin = value,
            Toggle::Improvements => plan.improvements = value,
            Toggle::AlphaCores => plan.alpha_cores = value,
            Toggle::InstallItems => plan.install_items = value,
        }
    }
}

/// The months and action log produced by simulating a plan.
type PlanOutcome = (Vec<Action>, i32);

/// Level 2 (inner): cost a fixed system plan by forced forward simulation on a
/// single shared timeline.
///
/// Returns the resulting action log and total months waited, or `None` if the
/// plan stalls or can't reach the goal within the safety caps.
pub(crate) fn simulate_plan(
    initial_state: &State,
    goal: &Goal,
    plan: &SystemPlan,
    slim: bool,
) -> Option<PlanOutcome> {
    let mut state = initial_state.clone();
    let mut total_months: i32 = 0;
    let mut iters: u32 = 0;

    loop {
        if goal.is_satisfied_quiet(&state) {
            return Some((state.action_log().clone(), total_months));
        }

        iters += 1;
        if iters > MAX_SIM_ITERS || total_months > MAX_PLAN_MONTHS {
            return None;
        }

        let actions = state.get_ordered_possible_actions(slim);

        // 1) Take any immediately-applicable plan action first (the list is
        //    already sorted by the action priority used elsewhere).
        if let Some(action) = actions
            .iter()
            .find(|a| !is_wait(a) && plan.allows(a))
            .cloned()
        {
            state.apply_action_raw(&action, false);
            continue;
        }

        // 2) Nothing to do right now: advance to the next meaningful event.
        //    A single Wait ticks every colonized planet at once.
        let mut wait: Option<u32> = None;
        for action in &actions {
            if let Action::Wait(months) = action {
                if *months > 0 {
                    wait = Some(wait.map_or(*months, |w| w.min(*months)));
                }
            }
        }
        if let Some(credit_months) = credit_wait(&state, plan) {
            wait = Some(wait.map_or(credit_months, |w| w.min(credit_months)));
        }

        match wait {
            Some(months) if months > 0 => {
                state.apply_action_raw(&Action::Wait(months), false);
                total_months += months as i32;
            }
            _ => {
                // No event to wait for. If income is positive and targets
                // remain, nudge one month (growth may unlock requirements);
                // otherwise the plan can never make progress.
                if state.balance().net_income() > 0.0 && has_pending_target(&state, plan) {
                    state.apply_action_raw(&Action::Wait(1), false);
                    total_months += 1;
                } else {
                    return None;
                }
            }
        }
    }
}

/// Months of waiting needed to afford the cheapest pending plan target
/// (colonization, or an unbuilt plan facility) at the current *system* net
/// income. `None` if nothing is pending or income can't fund anything.
fn credit_wait(state: &State, plan: &SystemPlan) -> Option<u32> {
    let credits = state.balance().credits();
    let net = state.balance().net_income();
    if net <= 0.0 {
        return None;
    }

    let mut cheapest: Option<f64> = None;
    let mut consider = |cost: f64| {
        if cost > credits {
            cheapest = Some(cheapest.map_or(cost, |c: f64| c.min(cost)));
        }
    };

    for (hash, pplan) in &plan.planets {
        let Some(planet) = state.system().get_planet_by_hash(*hash) else {
            continue;
        };
        if pplan.colonize && !planet.has_colony() {
            consider(COLONIZE_COST);
        }
        if planet.has_colony() {
            for ft in &pplan.facilities {
                // A higher tier already present supersedes this one (e.g. a star
                // fortress covers the orbital station the plan also lists), so it
                // is not a pending build.
                if !planet.has_facility_or_upgrade(*ft) {
                    if let Some(data) = FACILITY_DATA.get(ft) {
                        consider(data.build_cost as f64);
                    }
                }
            }
        }
    }

    cheapest.map(|cost| (((cost - credits) / net).ceil()).max(1.0) as u32)
}

/// Whether the plan still has something left to colonize/build anywhere.
fn has_pending_target(state: &State, plan: &SystemPlan) -> bool {
    for (hash, pplan) in &plan.planets {
        let Some(planet) = state.system().get_planet_by_hash(*hash) else {
            continue;
        };
        if pplan.colonize && !planet.has_colony() {
            return true;
        }
        if planet.has_colony()
            && pplan
                .facilities
                .iter()
                .any(|ft| !planet.has_facility_or_upgrade(*ft))
        {
            return true;
        }
    }
    false
}

/// A maximal per-planet plan: colonize (if needed), every toggle on, every
/// facility included. Seeding with *all* facilities (rather than only
/// metric-helping ones) guarantees prerequisite structures are available — a
/// Megaport is useless to a plan that forbade its Spaceport. The hill-climb then
/// trims toward the minimum makespan.
fn planet_plan_full(colonize: bool) -> PlanetPlan {
    PlanetPlan {
        colonize,
        free_port: true,
        hazard_pay: true,
        upgrade_admin: true,
        improvements: true,
        alpha_cores: true,
        install_items: true,
        facilities: FACILITY_DATA.keys().copied().collect(),
    }
}

/// Build a system plan where `active` planets get the full plan and all other
/// planets are left undeveloped. Pre-colonized planets are always active (their
/// colony can't be undone), so the meaningful choice is over uncolonized ones.
fn build_system_plan(state: &State, active: &HashSet<u64>) -> SystemPlan {
    let mut planets = HashMap::new();
    for (hash, planet) in state.system().planets() {
        let pplan = if active.contains(hash) {
            planet_plan_full(!planet.has_colony())
        } else {
            PlanetPlan::default()
        };
        planets.insert(*hash, pplan);
    }
    SystemPlan { planets }
}

/// Pick which planets to develop. A maximal "colonize everything" plan is often
/// infeasible for a system-wide goal: colonizing a weak planet drags down
/// average stability (and early net income) so the goal can never be met. So we
/// search over *which* planets to develop and return the cheapest feasible set
/// (with its simulated outcome), to seed the facility-level hill-climb.
///
/// For a small number of optional (uncolonized) planets we enumerate all
/// subsets; beyond that we fall back to greedily dropping one planet at a time.
fn choose_planet_set(
    state: &State,
    goal: &Goal,
    slim: bool,
    start: Instant,
    deadline: Duration,
    nodes_searched: &mut u32,
) -> Option<(SystemPlan, PlanOutcome)> {
    let forced: HashSet<u64> = state
        .system()
        .planets()
        .iter()
        .filter(|(_, p)| p.has_colony())
        .map(|(h, _)| *h)
        .collect();
    let optional: Vec<u64> = state
        .system()
        .planets()
        .iter()
        .filter(|(_, p)| !p.has_colony())
        .map(|(h, _)| *h)
        .collect();

    let mut best: Option<(SystemPlan, PlanOutcome)> = None;

    const MAX_ENUM_OPTIONAL: usize = 6;
    if optional.len() <= MAX_ENUM_OPTIONAL {
        // Enumerate every subset of the optional planets (always include forced).
        for mask in 0..(1u32 << optional.len()) {
            if start.elapsed() >= deadline {
                break;
            }
            let mut active = forced.clone();
            for (i, h) in optional.iter().enumerate() {
                if mask & (1 << i) != 0 {
                    active.insert(*h);
                }
            }
            if active.is_empty() {
                continue; // nothing colonized can't meet a positive goal
            }
            let plan = build_system_plan(state, &active);
            consider_plan(state, goal, slim, plan, &mut best, nodes_searched);
        }
    } else {
        // Greedy fallback: start from all planets and drop the one whose removal
        // improves the result, until no further improvement.
        let mut active: HashSet<u64> = forced.iter().copied().chain(optional.clone()).collect();
        let plan = build_system_plan(state, &active);
        consider_plan(state, goal, slim, plan, &mut best, nodes_searched);
        loop {
            if start.elapsed() >= deadline {
                break;
            }
            let mut dropped = None;
            for &h in &optional {
                if !active.contains(&h) {
                    continue;
                }
                let mut trial_active = active.clone();
                trial_active.remove(&h);
                if trial_active.is_empty() {
                    continue;
                }
                let before = best.as_ref().map(|(_, o)| o.1);
                let plan = build_system_plan(state, &trial_active);
                consider_plan(state, goal, slim, plan, &mut best, nodes_searched);
                let after = best.as_ref().map(|(_, o)| o.1);
                if after != before {
                    dropped = Some(h);
                    break;
                }
            }
            match dropped {
                Some(h) => {
                    active.remove(&h);
                }
                None => break,
            }
        }
    }

    best
}

/// Simulate `plan` and replace `best` if the outcome is better (fewer months, or
/// equal months with a smaller plan).
fn consider_plan(
    state: &State,
    goal: &Goal,
    slim: bool,
    plan: SystemPlan,
    best: &mut Option<(SystemPlan, PlanOutcome)>,
    nodes: &mut u32,
) {
    *nodes += 1;
    if let Some(out) = simulate_plan(state, goal, &plan, slim) {
        let better = best
            .as_ref()
            .map_or(true, |(bp, bo)| is_better(&out, &plan, bo, bp));
        if better {
            *best = Some((plan, out));
        }
    }
}

/// A candidate is better if it reaches the goal in fewer months, or in the same
/// months with a smaller plan.
fn is_better(
    cand: &PlanOutcome,
    cand_plan: &SystemPlan,
    best: &PlanOutcome,
    best_plan: &SystemPlan,
) -> bool {
    cand.1 < best.1 || (cand.1 == best.1 && cand_plan.size() < best_plan.size())
}

/// Level 1 (outer): two-level decomposition solve over the given state's planets
/// on a single shared timeline.
///
/// First [`choose_planet_set`] decides *which* planets to develop (a maximal
/// "colonize everything" plan is often infeasible for a system-wide goal). Then
/// the hill-climb refines that set by dropping per-planet facilities and turning
/// per-planet toggles off, accepting any change that doesn't increase months.
/// Bounded by `time_limit` (ms); returns the best plan found, or `None` if no
/// subset of planets can reach the goal.
pub fn decomp_search(
    initial_state: &mut State,
    goal: &Goal,
    time_limit: u32,
    slim: bool,
) -> Option<AStarSearchResult> {
    let start = Instant::now();
    let deadline = Duration::from_millis(time_limit as u64);
    let mut nodes_searched: u32 = 0;

    // First decide which planets to develop, then refine facilities/toggles.
    let (mut best_plan, mut best) =
        choose_planet_set(initial_state, goal, slim, start, deadline, &mut nodes_searched)?;

    // Hill-climb: try dropping each facility / toggle; keep non-worsening moves.
    let mut improved = true;
    while improved && start.elapsed() < deadline {
        improved = false;

        // Per-planet facility removals.
        let facs: Vec<(u64, FacilityType)> = best_plan
            .planets
            .iter()
            .flat_map(|(h, p)| p.facilities.iter().map(move |ft| (*h, *ft)))
            .collect();
        for (h, ft) in facs {
            if start.elapsed() >= deadline {
                break;
            }
            let mut trial = best_plan.clone();
            if let Some(pp) = trial.planets.get_mut(&h) {
                pp.facilities.remove(&ft);
            }
            nodes_searched += 1;
            if let Some(cand) = simulate_plan(initial_state, goal, &trial, slim) {
                if is_better(&cand, &trial, &best, &best_plan) {
                    best = cand;
                    best_plan = trial;
                    improved = true;
                }
            }
        }

        // Per-planet toggle removals.
        let hashes: Vec<u64> = best_plan.planets.keys().copied().collect();
        for h in hashes {
            for toggle in Toggle::ALL {
                if start.elapsed() >= deadline {
                    break;
                }
                if !toggle.get(&best_plan.planets[&h]) {
                    continue;
                }
                let mut trial = best_plan.clone();
                toggle.set(trial.planets.get_mut(&h).unwrap(), false);
                nodes_searched += 1;
                if let Some(cand) = simulate_plan(initial_state, goal, &trial, slim) {
                    if is_better(&cand, &trial, &best, &best_plan) {
                        best = cand;
                        best_plan = trial;
                        improved = true;
                    }
                }
            }
        }
    }

    let (solution, months) = best;
    Some(AStarSearchResult {
        solution: Some(solution),
        cost: months,
        cutoff_occurred: false,
        nodes_searched,
        nodes_pruned_by_bound: 0,
    })
}

/// Joint solve: optimize one combined plan over the whole system on a shared
/// timeline and budget. Returns a single result (wrapped in a `Vec` for
/// drop-in symmetry with the other entry points), or empty if unreachable.
pub fn search_system_decomp(
    initial_state: &mut State,
    goal: &Goal,
    time_limit: u32,
    slim: bool,
) -> Vec<AStarSearchResult> {
    decomp_search(initial_state, goal, time_limit, slim)
        .into_iter()
        .collect()
}
