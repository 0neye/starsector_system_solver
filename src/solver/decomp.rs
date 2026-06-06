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
//!
//! The same two-level machinery also runs a *maximize* objective
//! ([`search_system_maximize`]): instead of minimizing months to a threshold, it
//! pushes one [`Metric`] as high as possible within a month horizon while holding
//! the other metrics above floors. Only the Level-2 stop condition and the
//! Level-1 ranking ([`is_better`]) differ by [`Objective`]; the plan search,
//! resource sharing and forced schedule are shared verbatim.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::constants::{FacilityType, FACILITY_DATA};
use crate::solver::goal::{AStarSearchResult, Goal, Metric};
use crate::solver::state::{Action, State};

/// What the search optimizes.
///
/// * `Reach` is the original behavior: minimize the months needed to satisfy a
///   threshold [`Goal`].
/// * `Maximize` finds the plan whose reachable state has the highest value of
///   one [`Metric`] while keeping the *other* metrics above the floors in
///   `floors`, evaluated over instants up to `horizon_months` game-months. Within
///   that horizon a faster build-out can win by reaching higher growth sooner, so
///   the same hill-climb applies — only the ranking direction changes.
pub enum Objective<'a> {
    Reach(&'a Goal),
    Maximize {
        metric: Metric,
        floors: &'a Goal,
        horizon_months: i32,
    },
}

impl Objective<'_> {
    /// The constraints every reported instant must satisfy. In `Reach` mode the
    /// goal *is* the constraint; in `Maximize` mode these are the floors on the
    /// non-maximized metrics.
    fn floors(&self) -> &Goal {
        match self {
            Objective::Reach(goal) => goal,
            Objective::Maximize { floors, .. } => floors,
        }
    }

    /// The last month the simulation evaluates. `Reach` runs to the safety cap;
    /// `Maximize` stops at the caller's budget.
    fn horizon_months(&self) -> i32 {
        match self {
            Objective::Reach(_) => MAX_PLAN_MONTHS,
            Objective::Maximize { horizon_months, .. } => *horizon_months,
        }
    }

    /// The metric to maximize, or `None` in `Reach` mode (where the first
    /// satisfying instant wins and no value is tracked).
    fn metric(&self) -> Option<Metric> {
        match self {
            Objective::Reach(_) => None,
            Objective::Maximize { metric, .. } => Some(*metric),
        }
    }
}

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

/// The full result of simulating a plan, including how close an *infeasible*
/// plan came to the goal. The outer search ranks by [`is_better`]: feasible
/// plans beat infeasible ones, feasible plans compete on months (then plan
/// size), and infeasible plans compete on [`PlanScore::violation`] so the
/// hill-climb can descend from an infeasible seed toward the feasible region.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlanScore {
    feasible: bool,
    months: i32,
    /// The smallest instantaneous [`goal_violation`] seen over the simulated
    /// timeline; `0.0` exactly when the plan satisfies the goal at some instant.
    violation: f64,
    /// In maximize mode, the value of the maximized metric at the reported
    /// instant (the best feasible instant within the horizon); `f64::NEG_INFINITY`
    /// for infeasible plans and unused (`0.0`) in reach mode.
    value: f64,
    /// The action log at the satisfying instant (feasible) or empty (infeasible,
    /// where only the violation matters for ranking).
    log: Vec<Action>,
}

/// Instantaneous, normalized distance from `state` to satisfying `goal`: the sum
/// of each constraint's relative shortfall. Zero exactly when every constraint
/// the goal sets is met simultaneously — matching [`Goal::is_satisfied_quiet`].
///
/// Normalizing each term by its threshold keeps income (tens of thousands),
/// stability (single digits) and defense comparable, so the hill-climb doesn't
/// fixate on the largest-magnitude constraint.
fn goal_violation(goal: &Goal, state: &State) -> f64 {
    let mut v = 0.0;

    let income = state.balance().net_income();
    if income < goal.min_net_income {
        v += (goal.min_net_income - income) / goal.min_net_income.abs().max(1.0);
    }

    if let Some(min_defense) = goal.min_ground_defense {
        let defense = state.system().avg_ground_defense();
        if defense < min_defense {
            v += (min_defense - defense) / min_defense.abs().max(1.0);
        }
    }

    if let Some(min_stability) = goal.min_stability {
        let stability = state.system().avg_stability();
        let min_stability = min_stability as f64;
        if stability < min_stability {
            v += (min_stability - stability) / min_stability.abs().max(1.0);
        }
    }

    v
}

/// Level 2 (inner): cost a fixed system plan by forced forward simulation on a
/// single shared timeline.
///
/// Returns the resulting action log and total months waited, or `None` if the
/// plan stalls or can't reach the goal within the safety caps. Thin wrapper over
/// [`run_plan`] preserving the feasible-only contract the test suite relies on.
pub(crate) fn simulate_plan(
    initial_state: &State,
    goal: &Goal,
    plan: &SystemPlan,
    slim: bool,
) -> Option<PlanOutcome> {
    let score = run_plan(initial_state, &Objective::Reach(goal), plan, slim);
    score.feasible.then_some((score.log, score.months))
}

/// Forced forward simulation of a fixed plan, returning a full [`PlanScore`].
///
/// Reach and maximize modes share the build-ASAP/wait-to-next-event schedule and
/// both track the minimum goal violation so infeasible plans carry a gradient.
/// They differ only in what a *feasible* instant means:
///
/// * **Reach**: the first instant the goal is met wins — return immediately with
///   the months taken.
/// * **Maximize**: keep simulating to the horizon, and among every instant whose
///   floors are met, remember the one with the highest metric value (ties broken
///   toward fewer months). Waits are clamped so the metric is read no later than
///   the horizon.
fn run_plan(initial_state: &State, objective: &Objective, plan: &SystemPlan, slim: bool) -> PlanScore {
    let floors = objective.floors();
    let horizon = objective.horizon_months();
    let metric = objective.metric();

    let mut state = initial_state.clone();
    let mut total_months: i32 = 0;
    let mut iters: u32 = 0;
    let mut min_violation = f64::INFINITY;

    // Best feasible instant seen so far. Reach mode returns on the first; maximize
    // mode accumulates the highest-value one across the horizon.
    let mut best_value = f64::NEG_INFINITY;
    let mut best_months = 0;
    let mut best_log: Option<Vec<Action>> = None;

    let infeasible = |months: i32, violation: f64| PlanScore {
        feasible: false,
        months,
        violation,
        value: f64::NEG_INFINITY,
        log: Vec::new(),
    };

    // Maximize mode never simulates past the horizon, so the metric is read
    // exactly at the budget boundary; reach mode is unconstrained here.
    let clamp_wait = |months: u32, total: i32| -> u32 {
        if metric.is_some() {
            months.min((horizon - total).max(0) as u32)
        } else {
            months
        }
    };

    loop {
        let violation = goal_violation(floors, &state);
        if violation <= 0.0 {
            match metric {
                None => {
                    // Reach: the first satisfying instant is the answer.
                    return PlanScore {
                        feasible: true,
                        months: total_months,
                        violation: 0.0,
                        value: 0.0,
                        log: state.action_log().clone(),
                    };
                }
                Some(metric) => {
                    let value = metric.value(&state);
                    if best_log.is_none()
                        || value > best_value
                        || (value == best_value && total_months < best_months)
                    {
                        best_value = value;
                        best_months = total_months;
                        best_log = Some(state.action_log().clone());
                    }
                }
            }
        } else {
            min_violation = min_violation.min(violation);
        }

        iters += 1;
        if iters > MAX_SIM_ITERS || total_months > horizon {
            break;
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
            Some(months) if clamp_wait(months, total_months) > 0 => {
                let months = clamp_wait(months, total_months);
                state.apply_action_raw(&Action::Wait(months), false);
                total_months += months as i32;
            }
            _ => {
                // No event to wait for. If income is positive and targets
                // remain, nudge one month (growth may unlock requirements);
                // otherwise the plan can make no further progress.
                let step = clamp_wait(1, total_months);
                if step > 0 && state.balance().net_income() > 0.0 && has_pending_target(&state, plan)
                {
                    state.apply_action_raw(&Action::Wait(step), false);
                    total_months += step as i32;
                } else {
                    break;
                }
            }
        }
    }

    match best_log {
        Some(log) => PlanScore {
            feasible: true,
            months: best_months,
            violation: 0.0,
            value: best_value,
            log,
        },
        None => infeasible(total_months, min_violation),
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
///
/// `free_port` is seeded separately because it is the one lever that *trades*
/// objectives: it raises income but costs 1–3 stability (and lowers ground
/// defense via stability). Forcing it on makes a maximal plan infeasible under a
/// stability/defense floor, which is exactly the seed the pure-descent climb
/// can't recover from — so under such a floor we seed it off and let the climb
/// add it back where it pays for itself.
fn planet_plan_full(colonize: bool, free_port: bool) -> PlanetPlan {
    PlanetPlan {
        colonize,
        free_port,
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
fn build_system_plan(state: &State, active: &HashSet<u64>, free_port: bool) -> SystemPlan {
    let mut planets = HashMap::new();
    for (hash, planet) in state.system().planets() {
        let pplan = if active.contains(hash) {
            planet_plan_full(!planet.has_colony(), free_port)
        } else {
            PlanetPlan::default()
        };
        planets.insert(*hash, pplan);
    }
    SystemPlan { planets }
}

/// Free-port seed values to try for the maximal plan. With no stability/defense
/// floor, free port only helps (more income → fewer months) and the climb can
/// still drop it, so on-only is enough. With such a floor, the all-on seed may
/// be infeasible while free-port-off is feasible, so we must try both.
fn seed_free_ports(goal: &Goal) -> &'static [bool] {
    if goal.min_stability.is_some() || goal.min_ground_defense.is_some() {
        &[false, true]
    } else {
        &[true]
    }
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
    objective: &Objective,
    slim: bool,
    start: Instant,
    deadline: Duration,
    nodes_searched: &mut u32,
) -> Option<(SystemPlan, PlanScore)> {
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

    let mut best: Option<(SystemPlan, PlanScore)> = None;

    let free_ports = seed_free_ports(objective.floors());

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
            for &fp in free_ports {
                let plan = build_system_plan(state, &active, fp);
                consider_plan(state, objective, slim, plan, &mut best, nodes_searched);
            }
        }
    } else {
        // Greedy fallback: start from all planets and drop the one whose removal
        // improves the result, until no further improvement.
        let mut active: HashSet<u64> = forced.iter().copied().chain(optional.clone()).collect();
        for &fp in free_ports {
            let plan = build_system_plan(state, &active, fp);
            consider_plan(state, objective, slim, plan, &mut best, nodes_searched);
        }
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
                let before = best.as_ref().map(|(_, o)| (o.feasible, o.months, o.violation));
                for &fp in free_ports {
                    let plan = build_system_plan(state, &trial_active, fp);
                    consider_plan(state, objective, slim, plan, &mut best, nodes_searched);
                }
                let after = best.as_ref().map(|(_, o)| (o.feasible, o.months, o.violation));
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

/// Simulate `plan` and replace `best` if its score is better (see [`is_better`]).
/// Unlike a feasible-only filter, infeasible plans are retained and ranked by
/// violation, so the seed search hands the climb the *least-infeasible* plan when
/// nothing is outright feasible — the climb can then repair it.
fn consider_plan(
    state: &State,
    objective: &Objective,
    slim: bool,
    plan: SystemPlan,
    best: &mut Option<(SystemPlan, PlanScore)>,
    nodes: &mut u32,
) {
    *nodes += 1;
    let score = run_plan(state, objective, &plan, slim);
    let better = best
        .as_ref()
        .map_or(true, |(bp, bo)| is_better(objective, &score, &plan, bo, bp));
    if better {
        *best = Some((plan, score));
    }
}

/// Ranking between two plan scores (better wins):
/// * a feasible plan always beats an infeasible one;
/// * two feasible plans compete on the objective — reach minimizes months then
///   plan size, maximize takes the higher metric value then fewer months then
///   smaller plan;
/// * two infeasible plans compete on [`PlanScore::violation`] — how close they
///   came — so the hill-climb has a gradient to follow out of the infeasible
///   region (e.g. toggling free port off to recover the stability it lacks).
fn is_better(
    objective: &Objective,
    cand: &PlanScore,
    cand_plan: &SystemPlan,
    best: &PlanScore,
    best_plan: &SystemPlan,
) -> bool {
    match (cand.feasible, best.feasible) {
        (true, false) => true,
        (false, true) => false,
        (true, true) => match objective {
            Objective::Reach(_) => {
                cand.months < best.months
                    || (cand.months == best.months && cand_plan.size() < best_plan.size())
            }
            Objective::Maximize { .. } => {
                cand.value > best.value
                    || (cand.value == best.value && cand.months < best.months)
                    || (cand.value == best.value
                        && cand.months == best.months
                        && cand_plan.size() < best_plan.size())
            }
        },
        (false, false) => cand.violation < best.violation,
    }
}

/// Level 1 (outer): two-level decomposition solve over the given state's planets
/// on a single shared timeline, optimizing `objective`.
///
/// First [`choose_planet_set`] decides *which* planets to develop (a maximal
/// "colonize everything" plan is often infeasible for a system-wide goal). Then
/// the hill-climb refines that set by dropping per-planet facilities and turning
/// per-planet toggles off, accepting any change [`is_better`] approves under the
/// objective. Bounded by `time_limit` (ms); returns the best plan found, or
/// `None` if no subset of planets satisfies the objective's floors.
fn decomp_search_objective(
    initial_state: &mut State,
    objective: &Objective,
    time_limit: u32,
    slim: bool,
) -> Option<AStarSearchResult> {
    let start = Instant::now();
    let deadline = Duration::from_millis(time_limit as u64);
    let mut nodes_searched: u32 = 0;

    // First decide which planets to develop, then refine facilities/toggles.
    let (mut best_plan, mut best) =
        choose_planet_set(initial_state, objective, slim, start, deadline, &mut nodes_searched)?;

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
            let cand = run_plan(initial_state, objective, &trial, slim);
            if is_better(objective, &cand, &trial, &best, &best_plan) {
                best = cand;
                best_plan = trial;
                improved = true;
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
                let cand = run_plan(initial_state, objective, &trial, slim);
                if is_better(objective, &cand, &trial, &best, &best_plan) {
                    best = cand;
                    best_plan = trial;
                    improved = true;
                }
            }
        }

        // Free port is the one bidirectional lever: the loop above can only turn
        // it *off*, but a free-port-off seed (used under a stability/defense
        // floor) needs to turn it back *on* where the extra income shortens the
        // makespan without breaking the floor. Try flipping it to the opposite
        // of its current value on each planet.
        let hashes: Vec<u64> = best_plan.planets.keys().copied().collect();
        for h in hashes {
            if start.elapsed() >= deadline {
                break;
            }
            // Only meaningful for planets the plan actually develops.
            if best_plan.planets[&h].size() == 0 {
                continue;
            }
            let mut trial = best_plan.clone();
            let pp = trial.planets.get_mut(&h).unwrap();
            pp.free_port = !pp.free_port;
            nodes_searched += 1;
            let cand = run_plan(initial_state, objective, &trial, slim);
            if is_better(objective, &cand, &trial, &best, &best_plan) {
                best = cand;
                best_plan = trial;
                improved = true;
            }
        }
    }

    // The climb may have started from an infeasible seed and failed to repair it
    // (the goal is genuinely unreachable for any plan it reached). Only report a
    // solution that actually satisfies the goal.
    if !best.feasible {
        return None;
    }

    Some(AStarSearchResult {
        solution: Some(best.log),
        cost: best.months,
        cutoff_occurred: false,
        nodes_searched,
        nodes_pruned_by_bound: 0,
    })
}

/// Reach-mode entry: minimize the months needed to satisfy `goal`. Thin wrapper
/// over [`decomp_search_objective`] preserving the original public signature the
/// CLI and test suite call.
pub fn decomp_search(
    initial_state: &mut State,
    goal: &Goal,
    time_limit: u32,
    slim: bool,
) -> Option<AStarSearchResult> {
    decomp_search_objective(initial_state, &Objective::Reach(goal), time_limit, slim)
}

/// Maximize-mode entry: find the plan whose state has the highest `metric` value
/// within `horizon_months`, holding the other metrics above `floors`. The
/// returned [`AStarSearchResult::cost`] is the month at which that best value is
/// reached; replay the `solution` to read the achieved metric values.
pub fn decomp_search_maximize(
    initial_state: &mut State,
    metric: Metric,
    floors: &Goal,
    horizon_months: i32,
    time_limit: u32,
    slim: bool,
) -> Option<AStarSearchResult> {
    let objective = Objective::Maximize {
        metric,
        floors,
        horizon_months,
    };
    decomp_search_objective(initial_state, &objective, time_limit, slim)
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

/// Maximize-mode joint solve. Mirrors [`search_system_decomp`] but pushes one
/// `metric` as high as possible within `horizon_months` subject to `floors`.
pub fn search_system_maximize(
    initial_state: &mut State,
    metric: Metric,
    floors: &Goal,
    horizon_months: i32,
    time_limit: u32,
    slim: bool,
) -> Vec<AStarSearchResult> {
    decomp_search_maximize(initial_state, metric, floors, horizon_months, time_limit, slim)
        .into_iter()
        .collect()
}
