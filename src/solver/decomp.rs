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
//! system on a shared timeline and budget.
//!
//! The same two-level machinery also runs a *maximize* objective
//! ([`search_system_maximize`]): instead of minimizing months to a threshold, it
//! pushes one [`Metric`] as high as possible within a month horizon while holding
//! the other metrics above floors. Only the Level-2 stop condition and the
//! Level-1 ranking ([`is_better`]) differ by [`Objective`]; the plan search,
//! resource sharing and forced schedule are shared verbatim.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use rustc_hash::FxHashMap;

use crate::constants::{FacilityType, Resource, FACILITY_DATA, FACILITY_REQUIREMENTS};
use crate::planet::{upgrade_predecessors, Planet};
use crate::solver::goal::{AStarSearchResult, Goal, Metric};
use crate::solver::state::{Action, State};
use crate::solver::{FacilityUpgradePlacement, PlanSearch, SolverSettings};

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
/// Deterministic per-seed cap on plans evaluated by one hill-climb. This — not a
/// wall-clock deadline — is what stops a climb, so the result is identical on
/// every machine regardless of speed or load (the cause of the old maximize
/// nondeterminism was the wall clock interrupting a climb mid-pass; see
/// `workspace/OPTIMAL_SOLVER_BOUND.md`). Sized far above the ~20k a real climb needs to
/// converge, so it only bites for pathological inputs, and then deterministically.
const MAX_NODES_PER_SEED: u32 = 200_000;
const TOP_SEED_CLIMBS: usize = 8;

/// Search-effort profile: how many ranked seeds get climbed and how many plans
/// one climb may evaluate. Budgets are node counts, never wall-clock, so every
/// profile is deterministic. `FULL` reproduces the production search exactly;
/// the `QUICK_*` profiles trade quality for speed in the ranking mode and are
/// a strict *reduction* of `FULL` (same seed generation and queue order, lower
/// caps), so for one solve with the same seeds, a reduced profile's result is a
/// lower bound on `FULL`'s. (Across a chained sweep the warm seeds diverge, so
/// sweep-level scores are only empirically ordered.) See
/// `workspace/QUICK_RANKING_DESIGN.md`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SearchProfile {
    pub top_seed_climbs: usize,
    pub max_nodes_per_seed: u32,
    /// Repair mode: when extra (warm) seeds are supplied, skip planet-set seed
    /// generation and climb only from them. Falls back to full generation when
    /// no warm seed exists (e.g. a chain's first point).
    pub warm_seeds_only: bool,
}

impl SearchProfile {
    pub const FULL: Self = Self {
        top_seed_climbs: TOP_SEED_CLIMBS,
        max_nodes_per_seed: MAX_NODES_PER_SEED,
        warm_seeds_only: false,
    };
    /// Quick-ranking anchor point: full seeding, but only the best 2 seeds are
    /// climbed (the report showed most seeds converge into the same basins).
    pub const QUICK: Self = Self {
        top_seed_climbs: 2,
        max_nodes_per_seed: 50_000,
        warm_seeds_only: false,
    };
    /// Quick-ranking repair point: a mini-anchor. Full planet-set seeding plus
    /// the chain's warm plan, top-2 climbs at a reduced node cap. Warm-only
    /// 2k-node repairs compounded the anchor's error on large systems (Beta
    /// Bhel undershot the reference 22-34% per point with upgrades on).
    pub const QUICK_REPAIR: Self = Self {
        top_seed_climbs: 2,
        max_nodes_per_seed: 10_000,
        warm_seeds_only: false,
    };
    /// Bound-ranker per-planet solves: full seeding but a single climbed
    /// seed. The bound's solves are warm-chained (the previous floor's /
    /// allocation's plan is force-included past truncation), so the second
    /// climb mostly re-finds the same basin; dropping it roughly halves the
    /// per-solve cost. Income losses show up directly in the bound/full
    /// ratio gate, which is how this profile is validated.
    pub const BOUND: Self = Self {
        top_seed_climbs: 1,
        max_nodes_per_seed: MAX_NODES_PER_SEED,
        warm_seeds_only: false,
    };
    /// Tier-0 "instant paint": score the fixed template portfolio — the
    /// planet-set seeds (max-build-out plus each single-planet drop, in both
    /// free-port variants, see [`planet_set_seed_plans`]) — and take the best,
    /// with **no** hill-climb at all. `max_nodes_per_seed: 1` makes
    /// [`lazy_hill_climb`] return before evaluating a single neighbor, so each
    /// floor costs only one forced simulation per template (roughly linear in
    /// planets → milliseconds per system). The same templates run on every
    /// system, so the approximation error is correlated across systems, which
    /// is what rank preservation wants. Because it never climbs, its result for
    /// one solve lower-bounds `QUICK`'s from the same seeds (sweep-level scores
    /// are only empirically ordered — the warm chains diverge). See
    /// `workspace/QUICK_RANKING_DESIGN.md`.
    pub const TEMPLATE: Self = Self {
        top_seed_climbs: 1,
        max_nodes_per_seed: 1,
        warm_seeds_only: false,
    };

    /// Whether the evolutionary driver may engage for this profile. Only the
    /// full production search ([`SearchProfile::FULL`]) qualifies; the reduced
    /// ranking profiles (`QUICK`/`QUICK_REPAIR`/`BOUND`/`TEMPLATE`) always climb
    /// so ranking keeps its fast, budget-bounded behavior.
    pub(crate) fn permits_evolutionary(&self) -> bool {
        self.top_seed_climbs == TOP_SEED_CLIMBS && self.max_nodes_per_seed == MAX_NODES_PER_SEED
    }
}

/// Static facts about the search instance, computed once per solve and threaded
/// through seed generation and neighbor enumeration. Everything here is
/// immutable during a solve, so pruning on it is exact: a facility type outside
/// `relevant_facilities` can never be built on that planet (its deposit
/// requirements are fixed planet properties), and when the starting balance has
/// no colony items none can ever be acquired, so `InstallItems` toggles are
/// no-ops. Skipping such moves removes simulations that could not change the
/// outcome.
#[derive(Clone, Debug)]
struct SearchContext {
    relevant_facilities: HashMap<u64, Vec<FacilityType>>,
    allow_install_items: bool,
}

impl SearchContext {
    fn new(state: &State) -> Self {
        let relevant_facilities = state
            .system()
            .planets()
            .iter()
            .map(|(hash, planet)| (*hash, planet.statically_buildable_facilities()))
            .collect();
        Self {
            relevant_facilities,
            allow_install_items: !state.balance().colony_items().is_empty(),
        }
    }

    fn relevant_facilities(&self, hash: u64) -> &[FacilityType] {
        self.relevant_facilities
            .get(&hash)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

#[inline]
fn is_wait(action: &Action) -> bool {
    matches!(action, Action::Wait(_))
}

fn queue_allows_action(state: &State, action: &Action, settings: SolverSettings) -> bool {
    if settings.allow_parallel_builds {
        return true;
    }

    let Action::AddFacility(planet_hash, _) = action else {
        return true;
    };

    state
        .system()
        .get_planet_by_hash(*planet_hash)
        .is_none_or(|planet| {
            planet
                .facilities()
                .iter()
                .all(|facility| facility.remaining_build_days() <= 0)
        })
}

fn quality_mode() -> bool {
    static QUALITY: OnceLock<bool> = OnceLock::new();
    *QUALITY.get_or_init(|| {
        std::env::var_os("SYSTEM_SOLVER_QUALITY").is_some_and(|value| !value.is_empty())
    })
}

/// Candidate-score reuse across quiet sim months. Benchmarking showed it buys
/// only ~9% wall time after the factored lookahead landed, while its quality
/// impact had to be patched per-floor (it regressed the unconstrained-extreme
/// Pareto points). That floor-keyed guard generalizes badly to floors outside
/// the benchmark grid, so reuse is disabled until it can be justified without
/// special cases. The cache machinery and counters stay for future experiments.
fn score_reuse_enabled(_objective: &Objective) -> bool {
    false
}

/// Lightweight global counters for profiling the search (read/reset by the
/// benchmark harness via `SYSTEM_SOLVER_STATS=1`). Relaxed atomics; negligible
/// next to the work they count.
pub mod stats {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    pub static RUN_PLAN_CALLS: AtomicU64 = AtomicU64::new(0);
    pub static CACHE_HITS: AtomicU64 = AtomicU64::new(0);
    pub static SIM_STEPS: AtomicU64 = AtomicU64::new(0);
    pub static CAND_SCORES: AtomicU64 = AtomicU64::new(0);
    pub static SCORE_PASSES: AtomicU64 = AtomicU64::new(0);
    pub static REUSE_STEPS: AtomicU64 = AtomicU64::new(0);
    pub static SEEDS: AtomicU64 = AtomicU64::new(0);

    pub fn reset() {
        for c in [
            &RUN_PLAN_CALLS,
            &CACHE_HITS,
            &SIM_STEPS,
            &CAND_SCORES,
            &SCORE_PASSES,
            &REUSE_STEPS,
            &SEEDS,
        ] {
            c.store(0, Relaxed);
        }
    }

    /// (run_plan_calls, cache_hits, sim_steps, cand_scores, score_passes, reuse_steps, seeds)
    pub fn snapshot() -> (u64, u64, u64, u64, u64, u64, u64) {
        (
            RUN_PLAN_CALLS.load(Relaxed),
            CACHE_HITS.load(Relaxed),
            SIM_STEPS.load(Relaxed),
            CAND_SCORES.load(Relaxed),
            SCORE_PASSES.load(Relaxed),
            REUSE_STEPS.load(Relaxed),
            SEEDS.load(Relaxed),
        )
    }
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
    makeshift_comm_relay: bool,
}

impl SystemPlan {
    /// Whether this plan permits taking `action`. Legality/affordability is
    /// already enforced by the action generator; this only encodes plan intent.
    /// The planet is identified by the hash every non-`Wait` action carries.
    fn allows(&self, action: &Action) -> bool {
        match action {
            Action::AddFacility(h, ft) => self
                .planets
                .get(h)
                .is_some_and(|p| p.facilities.contains(ft)),
            Action::AddImprovement(h, _) => self.planets.get(h).is_some_and(|p| p.improvements),
            Action::AddAlphaCore(h, _) => self.planets.get(h).is_some_and(|p| p.alpha_cores),
            Action::InstallItem(h, _, _) => self.planets.get(h).is_some_and(|p| p.install_items),
            Action::SetFreePort(h, true) => self.planets.get(h).is_some_and(|p| p.free_port),
            Action::SetHazardPay(h, true) => self.planets.get(h).is_some_and(|p| p.hazard_pay),
            Action::UpgradeAdmin(h) => self.planets.get(h).is_some_and(|p| p.upgrade_admin),
            Action::BuildMakeshiftCommRelay => self.makeshift_comm_relay,
            Action::Colonize(h) => self.planets.get(h).is_some_and(|p| p.colonize),
            // The plan never deliberately disables a bonus.
            Action::SetFreePort(_, false) | Action::SetHazardPay(_, false) => false,
            Action::Wait(_) => false,
        }
    }

    fn permits_facility_upgrades(&self) -> bool {
        self.planets
            .values()
            .any(|p| p.improvements || p.alpha_cores)
    }

    /// A copy of this plan with every facility-improvement and alpha-core toggle
    /// cleared, so [`SystemPlan::allows`] rejects deferred upgrades and the
    /// greedy simulator never schedules one on its own. The two-pass discovery
    /// and replay passes both run over this masked plan; replay reintroduces the
    /// chosen upgrades explicitly via pin injection.
    fn without_facility_upgrades(&self) -> SystemPlan {
        let mut masked = self.clone();
        for plan in masked.planets.values_mut() {
            plan.improvements = false;
            plan.alpha_cores = false;
        }
        masked
    }

    /// Total number of decisions across all planets.
    fn size(&self) -> usize {
        self.planets.values().map(PlanetPlan::size).sum::<usize>()
            + usize::from(self.makeshift_comm_relay)
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
        SystemPlan {
            planets,
            makeshift_comm_relay: true,
        }
    }

    #[cfg(test)]
    pub(crate) fn set_upgrade_admin(&mut self, planet_hash: u64, value: bool) {
        if let Some(plan) = self.planets.get_mut(&planet_hash) {
            plan.upgrade_admin = value;
        }
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

#[derive(Default)]
struct PlanActionScoreCache {
    candidates: Vec<Action>,
    scores: HashMap<Action, ActionLookaheadScore>,
}

#[derive(Default)]
struct ChosenFacilityUpgrades {
    /// Chosen upgrades in allocation order (drives `ReplayAsap`/`LateApply`).
    order: Vec<Action>,
    /// Same actions in a set, so `insert` dedups in O(1).
    set: HashSet<Action>,
}

impl ChosenFacilityUpgrades {
    fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    fn insert(&mut self, action: &Action) {
        if is_deferred_facility_upgrade(action) && self.set.insert(action.clone()) {
            self.order.push(action.clone());
        }
    }

    /// The first chosen upgrade (in allocation order) that is legal right now —
    /// i.e. present in `actions` and not yet applied. Drives the ASAP replay:
    /// the simulator applies it before any ordinary plan action.
    fn next_applicable(&self, actions: &[Action]) -> Option<Action> {
        self.order
            .iter()
            .find(|chosen| actions.contains(chosen))
            .cloned()
    }
}

struct SimulationResult {
    score: PlanScore,
    final_state: State,
    final_months: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ScoreEventPlanet {
    hash: u64,
    size: u32,
    free_port_bucket: u32,
    built_flags: Vec<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ScoreEventSnapshot {
    planets: Vec<ScoreEventPlanet>,
}

impl ScoreEventSnapshot {
    fn from_state(state: &State) -> Self {
        let mut planets: Vec<_> = state
            .system()
            .planets()
            .values()
            .filter(|planet| planet.has_colony())
            .map(|planet| ScoreEventPlanet {
                hash: planet.name_hash(),
                size: planet.size(),
                free_port_bucket: planet.current_free_port_bucket(),
                built_flags: planet
                    .facilities()
                    .iter()
                    .map(|facility| facility.remaining_build_days() <= 0)
                    .collect(),
            })
            .collect();
        planets.sort_by_key(|planet| planet.hash);
        Self { planets }
    }
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
    exclude_upgrades: bool,
) -> Option<PlanOutcome> {
    simulate_plan_with_settings(
        initial_state,
        goal,
        plan,
        SolverSettings::legacy(!exclude_upgrades),
    )
}

pub(crate) fn simulate_plan_with_settings(
    initial_state: &State,
    goal: &Goal,
    plan: &SystemPlan,
    settings: SolverSettings,
) -> Option<PlanOutcome> {
    let score = run_plan(initial_state, &Objective::Reach(goal), plan, settings);
    score.feasible.then_some((score.log, score.months))
}

/// Maximize-mode counterpart of [`simulate_plan`]: score a fixed plan and return
/// the best `metric` value reached within `horizon_months` (with the month it was
/// reached), or `None` if the plan never satisfies `floors`. `pub(crate)` so the
/// test suite can assert per-plan properties (e.g. value is monotonic in the
/// horizon) without going through the heuristic outer search.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn simulate_plan_maximize(
    initial_state: &State,
    metric: Metric,
    floors: &Goal,
    horizon_months: i32,
    plan: &SystemPlan,
    exclude_upgrades: bool,
) -> Option<(f64, i32)> {
    let objective = Objective::Maximize {
        metric,
        floors,
        horizon_months,
    };
    let score = run_plan(
        initial_state,
        &objective,
        plan,
        SolverSettings::legacy(!exclude_upgrades),
    );
    score.feasible.then_some((score.value, score.months))
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn simulate_plan_maximize_with_log(
    initial_state: &State,
    metric: Metric,
    floors: &Goal,
    horizon_months: i32,
    plan: &SystemPlan,
    exclude_upgrades: bool,
) -> Option<(f64, i32, Vec<Action>)> {
    let objective = Objective::Maximize {
        metric,
        floors,
        horizon_months,
    };
    let score = run_plan(
        initial_state,
        &objective,
        plan,
        SolverSettings::legacy(!exclude_upgrades),
    );
    score
        .feasible
        .then_some((score.value, score.months, score.log))
}

#[cfg(test)]
pub(crate) fn simulate_plan_maximize_with_settings_and_log(
    initial_state: &State,
    metric: Metric,
    floors: &Goal,
    horizon_months: i32,
    plan: &SystemPlan,
    settings: SolverSettings,
) -> Option<(f64, i32, Vec<Action>)> {
    let objective = Objective::Maximize {
        metric,
        floors,
        horizon_months,
    };
    let score = run_plan(initial_state, &objective, plan, settings);
    score
        .feasible
        .then_some((score.value, score.months, score.log))
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
fn run_plan(
    initial_state: &State,
    objective: &Objective,
    plan: &SystemPlan,
    settings: SolverSettings,
) -> PlanScore {
    stats::RUN_PLAN_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if !should_use_two_pass_facility_upgrades(initial_state, objective, plan, settings) {
        return run_plan_once(initial_state, objective, plan, settings, None, false).score;
    }

    // Two-pass facility upgrades. Both passes run over a plan with deferred
    // upgrades masked off, so the greedy simulator only schedules the facility
    // builds; the upgrades are reintroduced deliberately once the full built
    // set is known.
    let masked = plan.without_facility_upgrades();

    // Pass 1 — discovery: build the plan out to the horizon (rolling past the
    // reach goal) to settle which facilities exist, then allocate upgrades onto
    // that settled state using the original (unmasked) plan's intent.
    let discovery = run_plan_once(initial_state, objective, &masked, settings, None, true);
    let chosen = choose_facility_upgrades(&discovery.final_state, objective, plan, settings);
    if chosen.is_empty() {
        return discovery.score;
    }

    if settings.facility_upgrade_placement == FacilityUpgradePlacement::LateApply {
        return apply_facility_upgrades_late(discovery, objective, &chosen);
    }

    // Pass 2 — replay: re-run the masked plan, injecting the chosen upgrades as
    // early as each becomes legal so the timeline reflects the boosts.
    run_plan_once(
        initial_state,
        objective,
        &masked,
        settings,
        Some(&chosen),
        false,
    )
    .score
}

fn should_use_two_pass_facility_upgrades(
    initial_state: &State,
    objective: &Objective,
    plan: &SystemPlan,
    settings: SolverSettings,
) -> bool {
    // Maximize only. The two-pass allocates scarce upgrades by scoring the
    // settled end state, which is exactly the maximize objective. For reach the
    // objective is minimum months, which the end state cannot see: once the
    // discovery state satisfies the floor, every upgrade is feasible with zero
    // violation and allocation degenerates to a final-income tie-break. That can
    // reserve the only story point for a high-income facility instead of the
    // improvement that would have met the floor earlier, inflating months and
    // breaking the reach contract. Single-pass greedy already applies the
    // violation-reducing upgrade as early as it is legal, so reach defers to it.
    objective.metric().is_some()
        && settings.include_industry_upgrades
        && settings.facility_upgrade_placement != FacilityUpgradePlacement::Greedy
        && plan.permits_facility_upgrades()
        && (initial_state.balance().story_points() >= 2
            || initial_state.balance().alpha_cores() > 0)
}

/// Forward-simulate a fixed plan once.
///
/// `pins`, when set, are facility upgrades to inject as early as each becomes
/// legal (the `ReplayAsap` second pass); the plan itself should have its
/// upgrade toggles masked off so the greedy never schedules an unpinned one.
/// `roll_to_end` keeps a reach simulation running past the first satisfying
/// instant so the discovery pass settles the full built set.
fn run_plan_once(
    initial_state: &State,
    objective: &Objective,
    plan: &SystemPlan,
    settings: SolverSettings,
    pins: Option<&ChosenFacilityUpgrades>,
    roll_to_end: bool,
) -> SimulationResult {
    let floors = objective.floors();
    let horizon = objective.horizon_months();
    let metric = objective.metric();

    let mut state = initial_state.clone();
    let mut total_months: i32 = 0;
    let mut iters: u32 = 0;
    let mut min_violation = f64::INFINITY;
    let mut score_cache = PlanActionScoreCache::default();
    let mut score_cache_dirty = true;
    let mut last_score_pass_month = i32::MIN / 2;

    // Best feasible instant seen so far. Reach mode returns on the first; maximize
    // mode accumulates the highest-value one across the horizon.
    let mut best_value = f64::NEG_INFINITY;
    let mut best_months = 0;
    let mut best_log_len: Option<usize> = None;

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
                    // Reach: the first satisfying instant is the answer, except
                    // discovery (`roll_to_end`) keeps rolling to learn the final
                    // build set.
                    if best_log_len.is_none() {
                        best_months = total_months;
                        best_log_len = Some(state.action_log().len());
                    }
                    if !roll_to_end {
                        return SimulationResult {
                            score: PlanScore {
                                feasible: true,
                                months: total_months,
                                violation: 0.0,
                                value: 0.0,
                                log: state.action_log().clone(),
                            },
                            final_state: state,
                            final_months: total_months,
                        };
                    }
                }
                Some(metric) => {
                    let value = metric.value(&state);
                    if best_log_len.is_none()
                        || maximize_improves(value, total_months, best_value, best_months)
                    {
                        best_value = value;
                        best_months = total_months;
                        best_log_len = Some(state.action_log().len());
                    }
                }
            }
        } else {
            min_violation = min_violation.min(violation);
        }

        iters += 1;
        stats::SIM_STEPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if iters > MAX_SIM_ITERS || total_months > horizon {
            break;
        }

        let actions = state.get_possible_actions(settings.exclude_upgrades());

        // 1) Replay: apply the next legal pinned facility upgrade immediately,
        //    in the discovered allocation order, ahead of any ordinary plan
        //    action. The masked plan rejects upgrades in `choose_plan_action`,
        //    so this injection is the only way a deferred upgrade enters replay.
        if let Some(action) = pins.and_then(|pins| pins.next_applicable(&actions)) {
            state.apply_action_raw(&action, false);
            continue;
        }

        // 2) Take the best immediately-applicable plan action. The static
        // action priority is only a tie-breaker here; for fixed-plan scheduling
        // we need to avoid building a generic multiplier (notably Commerce)
        // before the production it is supposed to multiply.
        if let Some(action) = choose_plan_action(
            &mut state,
            objective,
            plan,
            &actions,
            settings,
            &mut score_cache,
            &mut score_cache_dirty,
            total_months,
            &mut last_score_pass_month,
        ) {
            state.apply_action_raw(&action, false);
            continue;
        }

        // 3) Nothing to do right now: advance to the next meaningful event.
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
                let before = ScoreEventSnapshot::from_state(&state);
                state.apply_action_raw(&Action::Wait(months), false);
                let after = ScoreEventSnapshot::from_state(&state);
                score_cache_dirty |= before != after;
                total_months += months as i32;
            }
            _ => {
                // No event to wait for. If income is positive and targets
                // remain, nudge one month (growth may unlock requirements);
                // otherwise the plan can make no further progress.
                let step = clamp_wait(1, total_months);
                if step > 0
                    && state.balance().net_income() > 0.0
                    && has_pending_target(&state, plan)
                {
                    let before = ScoreEventSnapshot::from_state(&state);
                    state.apply_action_raw(&Action::Wait(step), false);
                    let after = ScoreEventSnapshot::from_state(&state);
                    score_cache_dirty |= before != after;
                    total_months += step as i32;
                } else {
                    break;
                }
            }
        }
    }

    let score = match best_log_len {
        Some(len) => PlanScore {
            feasible: true,
            months: best_months,
            violation: 0.0,
            value: metric.map_or(0.0, |_| best_value),
            log: state.action_log()[..len].to_vec(),
        },
        None => infeasible(total_months, min_violation),
    };

    SimulationResult {
        score,
        final_state: state,
        final_months: total_months,
    }
}

#[allow(clippy::too_many_arguments)] // threaded simulation context
fn choose_plan_action(
    state: &mut State,
    objective: &Objective,
    plan: &SystemPlan,
    actions: &[Action],
    settings: SolverSettings,
    cache: &mut PlanActionScoreCache,
    dirty: &mut bool,
    total_months: i32,
    last_score_pass_month: &mut i32,
) -> Option<Action> {
    // Score each candidate once via reversible apply/undo on the live state (no
    // per-candidate `State` clone — that clone was the run_plan hot-spot), then
    // pick the max. `max_by` would call the comparator O(n) times and recompute
    // both operands' scores on every call.
    let candidates: Vec<Action> = actions
        .iter()
        .filter(|a| !is_wait(a) && plan.allows(a) && queue_allows_action(state, a, settings))
        .cloned()
        .collect();

    if candidates.is_empty() {
        cache.candidates.clear();
        cache.scores.clear();
        *dirty = false;
        return None;
    }

    let reuse_enabled = score_reuse_enabled(objective);
    let force_full = !reuse_enabled
        || *dirty
        || cache.candidates.is_empty()
        || total_months - *last_score_pass_month >= 2;
    if force_full {
        let refs: Vec<_> = candidates.iter().collect();
        let scores = action_lookahead_scores(state, objective, &refs);
        cache.candidates = candidates.clone();
        cache.scores.clear();
        cache.scores.extend(candidates.iter().cloned().zip(scores));
        stats::SCORE_PASSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        *last_score_pass_month = total_months;
    } else {
        let missing: Vec<_> = candidates
            .iter()
            .filter(|action| !cache.scores.contains_key(*action))
            .collect();
        if !missing.is_empty() {
            let scores = action_lookahead_scores(state, objective, &missing);
            cache
                .scores
                .extend(missing.into_iter().cloned().zip(scores));
        }
        cache
            .scores
            .retain(|action, _| candidates.iter().any(|candidate| candidate == action));
        cache.candidates = candidates.clone();
        stats::REUSE_STEPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    *dirty = false;

    candidates
        .iter()
        .map(|a| {
            (
                *cache
                    .scores
                    .get(a)
                    .expect("score cache must cover the live candidate set"),
                a.clone(),
            )
        })
        .max_by(|(a_score, a), (b_score, b)| {
            a_score
                .cmp(b_score)
                // Preserve the old deterministic ordering as a final tie-break.
                .then_with(|| b.cmp(a))
                .then_with(|| b.get_hash().cmp(&a.get_hash()))
        })
        .map(|(_, a)| a)
}

fn is_deferred_facility_upgrade(action: &Action) -> bool {
    matches!(
        action,
        Action::AddImprovement(..) | Action::AddAlphaCore(..)
    )
}

/// For a maximize objective, whether outcome `(value_a, months_a)` strictly
/// beats `(value_b, months_b)`: higher value wins, ties broken by fewer months.
fn maximize_improves(value_a: f64, months_a: i32, value_b: f64, months_b: i32) -> bool {
    value_a > value_b || (value_a == value_b && months_a < months_b)
}

fn choose_facility_upgrades(
    discovery_state: &State,
    objective: &Objective,
    plan: &SystemPlan,
    settings: SolverSettings,
) -> ChosenFacilityUpgrades {
    let mut state = discovery_state.clone();
    let mut chosen = ChosenFacilityUpgrades::default();

    loop {
        let actions = state.get_possible_actions(settings.exclude_upgrades());
        let candidates: Vec<_> = actions
            .iter()
            .filter(|a| is_deferred_facility_upgrade(a) && plan.allows(a))
            .collect();
        if candidates.is_empty() {
            break;
        }

        let current_score = score_state(&state, objective, 0);
        let scores = action_lookahead_scores(&mut state, objective, &candidates);
        let best = candidates
            .into_iter()
            .zip(scores)
            .filter(|(_, score)| *score > current_score)
            .max_by(|(a_action, a_score), (b_action, b_score)| {
                a_score
                    .cmp(b_score)
                    .then_with(|| b_action.cmp(a_action))
                    .then_with(|| b_action.get_hash().cmp(&a_action.get_hash()))
            });

        let Some((action, _)) = best else {
            break;
        };
        let action = action.clone();
        state.apply_action_raw(&action, false);
        chosen.insert(&action);
    }

    chosen
}

/// Build the comparison score for `state` as it stands, attributing `earlier`
/// as the lookahead-delay tie-breaker (0 for the settled state, `-wait_months`
/// when scoring a hypothetical post-wait state).
fn score_state(state: &State, objective: &Objective, earlier: i32) -> ActionLookaheadScore {
    let violation = goal_violation(objective.floors(), state);
    let metric_value = objective
        .metric()
        .map_or_else(|| -violation, |metric| metric.value(state));

    ActionLookaheadScore {
        feasible: violation <= 0.0,
        objective_value: score_units(metric_value),
        floor_violation: score_units(violation),
        income: score_units(state.balance().net_income()),
        stability: score_units(state.system().avg_stability()),
        defense: score_units(state.system().avg_ground_defense()),
        earlier,
    }
}

fn apply_facility_upgrades_late(
    discovery: SimulationResult,
    objective: &Objective,
    chosen: &ChosenFacilityUpgrades,
) -> PlanScore {
    let mut state = discovery.final_state;
    for action in &chosen.order {
        state.apply_action_raw(action, false);
    }

    let violation = goal_violation(objective.floors(), &state);
    if violation <= 0.0 {
        let late = PlanScore {
            feasible: true,
            months: discovery.final_months,
            violation: 0.0,
            value: objective
                .metric()
                .map_or(0.0, |metric| metric.value(&state)),
            log: state.action_log().clone(),
        };

        return match objective {
            Objective::Reach(_) if discovery.score.feasible => discovery.score,
            Objective::Reach(_) => late,
            Objective::Maximize { .. } => {
                if discovery.score.feasible
                    && maximize_improves(
                        discovery.score.value,
                        discovery.score.months,
                        late.value,
                        late.months,
                    )
                {
                    discovery.score
                } else {
                    late
                }
            }
        };
    }

    if discovery.score.feasible {
        discovery.score
    } else {
        PlanScore {
            feasible: false,
            months: discovery.final_months,
            violation: discovery.score.violation.min(violation),
            value: f64::NEG_INFINITY,
            log: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActionLookaheadScore {
    feasible: bool,
    objective_value: i64,
    floor_violation: i64,
    income: i64,
    stability: i64,
    defense: i64,
    earlier: i32,
}

impl Ord for ActionLookaheadScore {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.feasible
            .cmp(&other.feasible)
            .then_with(|| self.objective_value.cmp(&other.objective_value))
            // Smaller violation is better.
            .then_with(|| other.floor_violation.cmp(&self.floor_violation))
            .then_with(|| self.income.cmp(&other.income))
            .then_with(|| self.stability.cmp(&other.stability))
            .then_with(|| self.defense.cmp(&other.defense))
            // Smaller lookahead delay is better.
            .then_with(|| self.earlier.cmp(&other.earlier))
    }
}

impl PartialOrd for ActionLookaheadScore {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Score the state that *would* result from taking `action` and waiting out
/// its natural build time (all planets advancing), then restore `state`
/// exactly.
fn action_lookahead_score(
    state: &mut State,
    objective: &Objective,
    action: &Action,
) -> ActionLookaheadScore {
    stats::CAND_SCORES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let original_planet = exact_scoring_restore_planet(state, action);
    state.apply_action_raw(action, false);

    let wait_months = natural_action_wait(action).min(objective.horizon_months().max(0) as u32);
    if wait_months > 0 {
        state.apply_action_raw(&Action::Wait(wait_months), false);
    }

    let score = score_state(state, objective, -(wait_months as i32));

    // Undo in the exact reverse order applied, restoring `state`.
    if wait_months > 0 {
        state.undo_last_action(false);
    }
    state.undo_last_action(false);
    if let Some((planet_hash, planet)) = original_planet {
        restore_planet_after_scoring(state, planet_hash, planet);
    }

    score
}

fn action_lookahead_scores(
    state: &mut State,
    objective: &Objective,
    actions: &[&Action],
) -> Vec<ActionLookaheadScore> {
    if actions.is_empty() {
        return Vec::new();
    }

    let horizon = objective.horizon_months().max(0) as u32;
    let mut waits: Vec<u32> = actions
        .iter()
        .filter_map(|action| {
            factored_planet(action).map(|_| natural_action_wait(action).min(horizon))
        })
        .collect();
    waits.sort_unstable();
    waits.dedup();

    if waits.is_empty() {
        return actions
            .iter()
            .map(|action| action_lookahead_score_reference(state, objective, action))
            .collect();
    }

    #[cfg(test)]
    let before_base = state.get_deep_hash();
    let base = FactoredLookaheadBase::new(state, &waits);
    #[cfg(test)]
    assert_eq!(
        state.get_deep_hash(),
        before_base,
        "factored base precompute must restore state"
    );
    actions
        .iter()
        .map(|action| match factored_planet(action) {
            Some(planet_hash) => {
                let wait_months = natural_action_wait(action).min(horizon);
                #[cfg(test)]
                let before_candidate = state.get_deep_hash();
                let score = factored_single_planet_lookahead_score(
                    state,
                    objective,
                    action,
                    planet_hash,
                    wait_months,
                    &base,
                );
                #[cfg(test)]
                assert_eq!(
                    state.get_deep_hash(),
                    before_candidate,
                    "factored candidate scoring must restore state after {action:?}"
                );
                score
            }
            None => action_lookahead_score_reference(state, objective, action),
        })
        .collect()
}

/// The single planet whose score inputs `action` can change, when the action
/// factors into a one-planet input swap against cached system totals.
/// System-wide actions (comm relay pushes a stability bonus into every
/// planet) and `Wait` return `None` and take the reference scoring path.
fn factored_planet(action: &Action) -> Option<u64> {
    match action {
        Action::AddFacility(planet_hash, _)
        | Action::AddImprovement(planet_hash, _)
        | Action::AddAlphaCore(planet_hash, _)
        | Action::InstallItem(planet_hash, _, _)
        | Action::SetFreePort(planet_hash, _)
        | Action::SetHazardPay(planet_hash, _)
        | Action::UpgradeAdmin(planet_hash)
        | Action::Colonize(planet_hash) => Some(*planet_hash),
        Action::BuildMakeshiftCommRelay | Action::Wait(_) => None,
    }
}

fn action_lookahead_score_reference(
    state: &mut State,
    objective: &Objective,
    action: &Action,
) -> ActionLookaheadScore {
    action_lookahead_score(state, objective, action)
}

/// Per-planet score inputs in supply space: commodity export income is not
/// additive across planets under the market-share model, but weighted supply
/// is, so planets contribute supply vectors and `SystemScoreInputs` applies
/// the `value * modded / (sector + raw)` division once over the summed
/// totals. This keeps the one-planet input swap exact.
#[derive(Clone, Copy, Debug)]
struct PlanetScoreInputs {
    direct_income: f64,
    total_upkeep: f64,
    stability: i32,
    ground_defense: f64,
    colonized: bool,
    export_mask: u32,
    raw_supply: [f64; Resource::COUNT],
    modded_supply: [f64; Resource::COUNT],
}

impl PlanetScoreInputs {
    fn from_planet(planet: &Planet) -> Self {
        let economy = planet.economy();
        let mut raw_supply = [0.0f64; Resource::COUNT];
        let mut modded_supply = [0.0f64; Resource::COUNT];
        let mut export_mask = 0u32;
        for &(resource, raw, modded) in &economy.exports {
            raw_supply[resource as usize] = raw;
            modded_supply[resource as usize] = modded;
            export_mask |= 1u32 << resource as usize;
        }
        Self {
            direct_income: economy.direct_income,
            total_upkeep: economy.upkeep,
            stability: planet.stability(),
            ground_defense: planet.ground_defense_strength(),
            colonized: planet.has_colony(),
            export_mask,
            raw_supply,
            modded_supply,
        }
    }
}

struct FactoredLookaheadBase {
    waits: Vec<u32>,
    planet_hashes: Vec<u64>,
    by_wait: Vec<Vec<PlanetScoreInputs>>,
    /// `total_by_wait[w]` = sum of every planet's inputs at wait `waits[w]`,
    /// precomputed once so a candidate's combined inputs are `total - old + new`
    /// (O(1)) instead of re-summing all planets (O(planets)) per candidate.
    total_by_wait: Vec<SystemScoreInputs>,
}

impl FactoredLookaheadBase {
    fn new(state: &mut State, waits: &[u32]) -> Self {
        let planet_hashes: Vec<u64> = state
            .system()
            .planets()
            .values()
            .map(|p| p.name_hash())
            .collect();
        let mut by_wait = vec![Vec::with_capacity(planet_hashes.len()); waits.len()];

        for planet_hash in &planet_hashes {
            let planet = state
                .system_mut()
                .get_planet_mut_by_hash(*planet_hash)
                .expect("planet hash collected from system must resolve");
            let snapshot = planet.snapshot_wait_state();
            let mut elapsed = 0;
            for (wait_index, wait) in waits.iter().copied().enumerate() {
                while elapsed < wait {
                    if planet.has_colony() {
                        planet.wait(1, false);
                    }
                    elapsed += 1;
                }
                by_wait[wait_index].push(PlanetScoreInputs::from_planet(planet));
            }
            planet.restore_wait_state(&snapshot);
        }

        let total_by_wait: Vec<SystemScoreInputs> = by_wait
            .iter()
            .map(|planets| {
                let mut total = SystemScoreInputs::default();
                for inputs in planets {
                    total.add(inputs);
                }
                total
            })
            .collect();

        Self {
            waits: waits.to_vec(),
            planet_hashes,
            by_wait,
            total_by_wait,
        }
    }

    fn wait_index(&self, wait_months: u32) -> usize {
        self.waits
            .binary_search(&wait_months)
            .expect("candidate wait must exist in factored base")
    }

    fn planet_index(&self, planet_hash: u64) -> usize {
        self.planet_hashes
            .iter()
            .position(|hash| *hash == planet_hash)
            .expect("candidate planet must exist in factored base")
    }

    fn combined_score_inputs(
        &self,
        wait_index: usize,
        replace_index: usize,
        replacement: &PlanetScoreInputs,
    ) -> SystemScoreInputs {
        if incremental_sum_enabled() {
            // total - the replaced planet + its replacement. Mathematically equal
            // to re-summing all planets with one swapped; the float association
            // differs by sub-ULP amounts that `score_units` (×1000, rounded)
            // quantizes away, so the factored-vs-reference exactness check holds.
            let mut totals = self.total_by_wait[wait_index];
            totals.sub(&self.by_wait[wait_index][replace_index]);
            totals.add(replacement);
            totals
        } else {
            // Reference O(planets) recombination (A/B baseline for the speedup;
            // SYSTEM_SOLVER_NO_INCSUM=1).
            let mut totals = SystemScoreInputs::default();
            for (idx, inputs) in self.by_wait[wait_index].iter().enumerate() {
                totals.add(if idx == replace_index {
                    replacement
                } else {
                    inputs
                });
            }
            totals
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SystemScoreInputs {
    direct_income: f64,
    total_upkeep: f64,
    stability_sum: i32,
    colonized_count: i32,
    ground_defense_sum: f64,
    export_mask: u32,
    raw_supply: [f64; Resource::COUNT],
    modded_supply: [f64; Resource::COUNT],
}

impl SystemScoreInputs {
    fn add(&mut self, planet: &PlanetScoreInputs) {
        self.direct_income += planet.direct_income;
        self.total_upkeep += planet.total_upkeep;
        if planet.colonized {
            self.stability_sum += planet.stability;
            self.colonized_count += 1;
            self.ground_defense_sum += planet.ground_defense;
        }
        self.export_mask |= planet.export_mask;
        let mut mask = planet.export_mask;
        while mask != 0 {
            let index = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            self.raw_supply[index] += planet.raw_supply[index];
            self.modded_supply[index] += planet.modded_supply[index];
        }
    }

    /// Remove a planet's contribution previously folded in by [`add`]. The export
    /// mask is intentionally *not* cleared: another planet may still export the
    /// same resource, and a resource whose supply has dropped to zero contributes
    /// exactly zero export income, so keeping the (super)mask is correct. This
    /// lets a per-wait system total be precomputed once and reused for every
    /// single-planet swap as `total - old_planet + replacement`, turning the
    /// per-candidate recombination from O(planets) into O(1).
    fn sub(&mut self, planet: &PlanetScoreInputs) {
        self.direct_income -= planet.direct_income;
        self.total_upkeep -= planet.total_upkeep;
        if planet.colonized {
            self.stability_sum -= planet.stability;
            self.colonized_count -= 1;
            self.ground_defense_sum -= planet.ground_defense;
        }
        let mut mask = planet.export_mask;
        while mask != 0 {
            let index = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            self.raw_supply[index] -= planet.raw_supply[index];
            self.modded_supply[index] -= planet.modded_supply[index];
        }
    }

    fn net_income(&self) -> f64 {
        let mut gross_income = self.direct_income;
        let mut mask = self.export_mask;
        while mask != 0 {
            let index = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            let resource = Resource::ALL[index];
            gross_income += resource.market_value() as f64 * self.modded_supply[index]
                / (resource.sector_supply() as f64 + self.raw_supply[index]);
        }
        gross_income - self.total_upkeep
    }

    fn avg_stability(&self) -> f64 {
        if self.colonized_count == 0 {
            0.0
        } else {
            self.stability_sum as f64 / self.colonized_count as f64
        }
    }

    fn avg_ground_defense(&self) -> f64 {
        if self.colonized_count == 0 {
            0.0
        } else {
            self.ground_defense_sum / self.colonized_count as f64
        }
    }

    fn metric_value(&self, metric: Metric) -> f64 {
        match metric {
            Metric::Income => self.net_income(),
            Metric::Defense => self.avg_ground_defense(),
            Metric::Stability => self.avg_stability(),
        }
    }

    fn goal_violation(&self, goal: &Goal) -> f64 {
        let mut v = 0.0;

        let income = self.net_income();
        if income < goal.min_net_income {
            v += (goal.min_net_income - income) / goal.min_net_income.abs().max(1.0);
        }

        if let Some(min_defense) = goal.min_ground_defense {
            let defense = self.avg_ground_defense();
            if defense < min_defense {
                v += (min_defense - defense) / min_defense.abs().max(1.0);
            }
        }

        if let Some(min_stability) = goal.min_stability {
            let stability = self.avg_stability();
            let min_stability = min_stability as f64;
            if stability < min_stability {
                v += (min_stability - stability) / min_stability.abs().max(1.0);
            }
        }

        v
    }
}

fn factored_single_planet_lookahead_score(
    state: &mut State,
    objective: &Objective,
    action: &Action,
    planet_hash: u64,
    wait_months: u32,
    base: &FactoredLookaheadBase,
) -> ActionLookaheadScore {
    stats::CAND_SCORES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let original_planet = exact_scoring_restore_planet(state, action);
    // Skip the balance-income refresh on this apply/undo pair: the factored
    // score reads `PlanetScoreInputs`/`SystemScoreInputs`, never balance
    // income, and the undo restores the system before anyone reads income.
    state.apply_action_raw_for_scoring(action, false);

    let replacement = {
        let planet = state
            .system_mut()
            .get_planet_mut_by_hash(planet_hash)
            .expect("factored action planet must exist after applying action");
        if wait_months > 0 && planet.has_colony() {
            let snapshot = planet.snapshot_wait_state();
            planet.wait(wait_months, false);
            let inputs = PlanetScoreInputs::from_planet(planet);
            planet.restore_wait_state(&snapshot);
            inputs
        } else {
            PlanetScoreInputs::from_planet(planet)
        }
    };

    state.undo_last_action_for_scoring(false);
    if let Some((planet_hash, planet)) = original_planet {
        restore_planet_after_scoring(state, planet_hash, planet);
    }

    let wait_index = base.wait_index(wait_months);
    let planet_index = base.planet_index(planet_hash);
    let inputs = base.combined_score_inputs(wait_index, planet_index, &replacement);
    let floors = objective.floors();
    let violation = inputs.goal_violation(floors);
    let metric_value = objective
        .metric()
        .map_or_else(|| -violation, |metric| inputs.metric_value(metric));

    let score = ActionLookaheadScore {
        feasible: violation <= 0.0,
        objective_value: score_units(metric_value),
        floor_violation: score_units(violation),
        income: score_units(inputs.net_income()),
        stability: score_units(inputs.avg_stability()),
        defense: score_units(inputs.avg_ground_defense()),
        earlier: -(wait_months as i32),
    };

    score
}

fn exact_scoring_restore_planet(state: &State, action: &Action) -> Option<(u64, Planet)> {
    let Action::AddFacility(planet_hash, facility_type) = action else {
        return None;
    };
    let planet = state.system().get_planet_by_hash(*planet_hash)?;
    let is_upgrade = FACILITY_REQUIREMENTS
        .get(facility_type)
        .is_some_and(|reqs| {
            reqs.facilities
                .iter()
                .any(|req| planet.get_facility(*req).is_some())
        });
    if is_upgrade {
        Some((*planet_hash, planet.clone()))
    } else {
        None
    }
}

fn restore_planet_after_scoring(state: &mut State, planet_hash: u64, planet: Planet) {
    *state
        .system_mut()
        .get_planet_mut_by_hash(planet_hash)
        .expect("scored action planet must still exist") = planet;
    let (gross_income, total_upkeep) = state.system().gross_income_and_upkeep();
    state
        .balance_mut()
        .update_income(gross_income, gross_income - total_upkeep);
}

#[cfg(test)]
pub(crate) fn assert_factored_lookahead_matches_reference(
    initial_state: &State,
    metric: Metric,
    floors: &Goal,
    horizon_months: i32,
    plan: &SystemPlan,
    exclude_upgrades: bool,
    min_candidate_scores: usize,
) -> usize {
    let objective = Objective::Maximize {
        metric,
        floors,
        horizon_months,
    };
    let mut state = initial_state.clone();
    let mut total_months: i32 = 0;
    let mut iters: u32 = 0;
    let mut compared = 0;

    let clamp_wait =
        |months: u32, total: i32| -> u32 { months.min((horizon_months - total).max(0) as u32) };

    while iters <= MAX_SIM_ITERS && total_months <= horizon_months {
        iters += 1;
        let actions = state.get_possible_actions(exclude_upgrades);
        let candidates: Vec<_> = actions
            .iter()
            .filter(|a| !is_wait(a) && plan.allows(a))
            .collect();

        if !candidates.is_empty() {
            let before = state.get_deep_hash();
            let factored = action_lookahead_scores(&mut state, &objective, &candidates);
            assert_eq!(
                state.get_deep_hash(),
                before,
                "factored batch scoring must restore state"
            );

            for (action, factored_score) in candidates.iter().zip(factored.iter()) {
                let before = state.get_deep_hash();
                let reference = action_lookahead_score_reference(&mut state, &objective, action);
                assert_eq!(
                    state.get_deep_hash(),
                    before,
                    "reference scoring must restore state after {action:?}"
                );
                assert_eq!(
                    *factored_score, reference,
                    "factored lookahead score diverged for {action:?}"
                );
                compared += 1;
            }

            let chosen = candidates
                .iter()
                .zip(factored)
                .map(|(a, score)| (score, (*a).clone()))
                .max_by(|(a_score, a), (b_score, b)| {
                    a_score
                        .cmp(b_score)
                        .then_with(|| b.cmp(a))
                        .then_with(|| b.get_hash().cmp(&a.get_hash()))
                })
                .map(|(_, action)| action)
                .expect("non-empty candidates produce a chosen action");
            state.apply_action_raw(&chosen, false);
            continue;
        }

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
                let step = clamp_wait(1, total_months);
                if step > 0
                    && state.balance().net_income() > 0.0
                    && has_pending_target(&state, plan)
                {
                    state.apply_action_raw(&Action::Wait(step), false);
                    total_months += step as i32;
                } else {
                    break;
                }
            }
        }

        if compared >= min_candidate_scores && total_months >= horizon_months {
            break;
        }
    }

    assert!(
        compared >= min_candidate_scores,
        "differential lookahead test compared {compared} candidates, expected at least {min_candidate_scores}"
    );
    compared
}

fn natural_action_wait(action: &Action) -> u32 {
    match action {
        Action::AddFacility(_, ft) => FACILITY_DATA
            .get(ft)
            .map(|data| (data.build_time as f64 / 30.0).ceil() as u32)
            .unwrap_or(0),
        _ => 0,
    }
}

fn score_units(value: f64) -> i64 {
    (value * 1_000.0).round() as i64
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
/// *statically buildable* facility included (a type whose deposit requirements
/// this planet can never satisfy is left out — including it could never change
/// the simulation). Seeding with the full buildable set guarantees prerequisite
/// structures are available — a Megaport is useless to a plan that forbade its
/// Spaceport. The hill-climb then trims toward the optimum.
///
/// `free_port` is seeded separately because it is the one lever that *trades*
/// objectives: it raises income but costs 1–3 stability (and lowers ground
/// defense via stability). Forcing it on makes a maximal plan infeasible under a
/// stability/defense floor, which is exactly the seed the pure-descent climb
/// can't recover from — so under such a floor we seed it off and let the climb
/// add it back where it pays for itself.
fn planet_plan_full(relevant: &[FacilityType], colonize: bool, free_port: bool) -> PlanetPlan {
    PlanetPlan {
        colonize,
        free_port,
        hazard_pay: true,
        upgrade_admin: true,
        improvements: true,
        alpha_cores: true,
        install_items: true,
        facilities: relevant.iter().copied().collect(),
    }
}

/// Build a system plan where `active` planets get the full plan and all other
/// planets are left undeveloped. Pre-colonized planets are always active (their
/// colony can't be undone), so the meaningful choice is over uncolonized ones.
fn build_system_plan(
    state: &State,
    ctx: &SearchContext,
    active: &HashSet<u64>,
    free_port: bool,
) -> SystemPlan {
    let mut planets = HashMap::new();
    for (hash, planet) in state.system().planets() {
        let pplan = if active.contains(hash) {
            planet_plan_full(
                ctx.relevant_facilities(*hash),
                !planet.has_colony(),
                free_port,
            )
        } else {
            PlanetPlan::default()
        };
        planets.insert(*hash, pplan);
    }
    SystemPlan {
        planets,
        makeshift_comm_relay: state.system().can_build_makeshift_comm_relay(),
    }
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
/// seed the all-planet set plus every one-planet drop (sorted for determinism)
/// and let the hill-climb refine the subset further via `Colonize` flip moves
/// (see [`one_move_neighbors`]) — the old exhaustive 2^n subset enumeration
/// climbed every subset independently, which dominated the solve cost on
/// systems with several uncolonized planets.
fn planet_set_seed_plans(
    state: &State,
    ctx: &SearchContext,
    objective: &Objective,
    start: Instant,
    deadline: Duration,
) -> Vec<SystemPlan> {
    let forced: HashSet<u64> = state
        .system()
        .planets()
        .iter()
        .filter(|(_, p)| p.has_colony())
        .map(|(h, _)| *h)
        .collect();
    let mut optional: Vec<u64> = state
        .system()
        .planets()
        .iter()
        .filter(|(_, p)| !p.has_colony())
        .map(|(h, _)| *h)
        .collect();
    optional.sort_unstable();

    let mut seeds = Vec::new();

    let free_ports = seed_free_ports(objective.floors());

    let active: HashSet<u64> = forced
        .iter()
        .copied()
        .chain(optional.iter().copied())
        .collect();
    if !active.is_empty() {
        for &fp in free_ports {
            seeds.push(build_system_plan(state, ctx, &active, fp));
        }
    }
    for &h in &optional {
        if start.elapsed() >= deadline {
            break;
        }
        let mut trial_active = active.clone();
        trial_active.remove(&h);
        if trial_active.is_empty() {
            continue;
        }
        for &fp in free_ports {
            seeds.push(build_system_plan(state, ctx, &trial_active, fp));
        }
    }

    seeds
}

/// Canonical encoding of a [`SystemPlan`] for memoization: per planet, one u64
/// packing the seven toggles (bits 0..7) and the facility set (bit `7 + ft as
/// u64` per facility — 22 facility types fit comfortably), sorted by planet
/// hash. Two plans encode equal iff they permit exactly the same actions, so a
/// memo keyed on this is exact.
type PlanKey = (Vec<(u64, u64)>, bool);

/// Cache key: the plan structure plus the facility-upgrade placement. [`run_plan`]
/// is a pure deterministic function of `(initial_state, objective, plan, settings)`;
/// within one solve the state and objective are fixed, but the surrogate-evaluated
/// EA scores the same plan under two placements (cheap `LateApply` for offspring,
/// accurate `ReplayAsap` for finalists), so the placement must be part of the key
/// or the two would collide. The other `SolverSettings` fields are constant per
/// solve and so need not be keyed.
type CacheKey = (PlanKey, FacilityUpgradePlacement);

/// Shared memo of plan -> simulated score for one solve. The seeds' climbs overlap
/// heavily (they converge into similar plans), which is where the hits come from.
type PlanCache = Mutex<FxHashMap<CacheKey, PlanScore>>;

fn plan_key(plan: &SystemPlan) -> PlanKey {
    let mut planets: Vec<(u64, u64)> = plan
        .planets
        .iter()
        .map(|(hash, p)| {
            let mut bits: u64 = 0;
            for (i, toggle) in Toggle::ALL.iter().enumerate() {
                if toggle.get(p) {
                    bits |= 1 << i;
                }
            }
            for ft in &p.facilities {
                bits |= 1 << (7 + *ft as u64);
            }
            (*hash, bits)
        })
        .collect();
    planets.sort_unstable_by_key(|(hash, _)| *hash);
    (planets, plan.makeshift_comm_relay)
}

/// [`run_plan`] through the shared memo. Hits still count as searched nodes at
/// the call sites, so budget-bound decisions are unchanged.
fn run_plan_cached(
    state: &State,
    objective: &Objective,
    plan: &SystemPlan,
    settings: SolverSettings,
    cache: &PlanCache,
) -> PlanScore {
    let key = (plan_key(plan), settings.facility_upgrade_placement);
    if let Some(score) = cache.lock().unwrap().get(&key) {
        stats::CACHE_HITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return score.clone();
    }
    let score = run_plan(state, objective, plan, settings);
    cache.lock().unwrap().insert(key, score.clone());
    score
}

/// Simulate `plan` and replace `best` if its score is better (see [`is_better`]).
/// Unlike a feasible-only filter, infeasible plans are retained and ranked by
/// violation, so the seed search hands the climb the *least-infeasible* plan when
/// nothing is outright feasible — the climb can then repair it.
#[allow(clippy::too_many_arguments)] // threaded search context, mirrors `hill_climb`
fn climb_from_seed(
    state: &State,
    ctx: &SearchContext,
    objective: &Objective,
    settings: SolverSettings,
    plan: SystemPlan,
    node_budget: u32,
    cache: &PlanCache,
    start: Instant,
    deadline: Duration,
) -> (SystemPlan, PlanScore, u32) {
    let mut nodes = 1;
    let score = run_plan_cached(state, objective, &plan, settings, cache);
    let (plan, score) = lazy_hill_climb(
        state,
        ctx,
        objective,
        settings,
        plan,
        score,
        node_budget,
        &mut nodes,
        cache,
        start,
        deadline,
    );
    (plan, score, nodes)
}

/// Hard stop for the search loops: wall-clock deadline exceeded or a
/// cooperative cancel was requested ([`crate::solver::cancel`]). Polled at
/// node granularity — each node is a full plan simulation, so the
/// `Instant::now()` cost is negligible. When this fires mid-climb the climb
/// returns its best-so-far plan (best-effort, no longer machine-independent).
fn should_stop(start: Instant, deadline: Duration) -> bool {
    crate::solver::cancel::is_cancelled() || start.elapsed() >= deadline
}

fn choose_planet_set(
    state: &State,
    ctx: &SearchContext,
    objective: &Objective,
    settings: SolverSettings,
    start: Instant,
    deadline: Duration,
    nodes_searched: &mut u32,
) -> Option<(SystemPlan, PlanScore)> {
    let mut best: Option<(SystemPlan, PlanScore)> = None;
    for plan in planet_set_seed_plans(state, ctx, objective, start, deadline) {
        *nodes_searched += 1;
        let score = run_plan(state, objective, &plan, settings);
        let better = best
            .as_ref()
            .map_or(true, |(bp, bo)| is_better(objective, &score, &plan, bo, bp));
        if better {
            best = Some((plan, score));
        }
    }
    best
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

/// Whether `plan` develops the planet `hash`: it is pre-colonized (its colony is
/// permanent) or the plan elects to colonize it. Undeveloped planets are owned by
/// the planet-set seed ([`choose_planet_set`]), not the facility/toggle climb, so
/// the neighbor generators skip them.
fn plan_develops(state: &State, plan: &SystemPlan, hash: u64) -> bool {
    plan.planets.get(&hash).is_some_and(|p| p.colonize)
        || state
            .system()
            .get_planet_by_hash(hash)
            .is_some_and(|p| p.has_colony())
}

/// One plan edit the climb can make, identified independently of the plan it is
/// applied to. The lazy climb keys its priority queue on these; whether a key is
/// currently legal (and what it flips *to*) is decided by [`apply_move`] against
/// the plan of the moment, so stale queue entries are simply re-interpreted or
/// discarded instead of invalidating the queue on every accepted move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum MoveKey {
    RelayFlip,
    /// Flip the whole planet in/out of the plan (never for pre-colonized planets).
    ColonizeFlip(u64),
    FacDrop(u64, FacilityType),
    FacAdd(u64, FacilityType),
    /// Index into [`Toggle::ALL`]; never `Colonize` (that is `ColonizeFlip`).
    ToggleFlip(u64, u8),
}

fn move_locality(key: MoveKey) -> Option<u64> {
    match key {
        MoveKey::RelayFlip => None,
        MoveKey::ColonizeFlip(h)
        | MoveKey::FacDrop(h, _)
        | MoveKey::FacAdd(h, _)
        | MoveKey::ToggleFlip(h, _) => Some(h),
    }
}

fn move_facility_type(key: MoveKey) -> Option<FacilityType> {
    match key {
        MoveKey::FacDrop(_, ft) | MoveKey::FacAdd(_, ft) => Some(ft),
        MoveKey::RelayFlip | MoveKey::ColonizeFlip(_) | MoveKey::ToggleFlip(_, _) => None,
    }
}

fn wake_after_accept(accepted: MoveKey, parked: MoveKey) -> bool {
    if matches!(parked, MoveKey::ColonizeFlip(_) | MoveKey::RelayFlip) {
        return true;
    }

    if move_facility_type(accepted).is_some()
        && move_facility_type(accepted) == move_facility_type(parked)
    {
        return true;
    }

    match move_locality(accepted) {
        // A global accepted move can perturb every planet's score.
        None => true,
        Some(h) => move_locality(parked) == Some(h),
    }
}

/// All move keys for `plan`, in the canonical deterministic order: relay flip
/// first, then per planet (sorted by hash): colonize flip, facility drops
/// (sorted), facility adds (statically-buildable only, sorted), toggle flips.
/// Colonize flips let the climb refine the planet subset the seeds only sample.
/// `InstallItems` flips are skipped when the starting balance has no colony
/// items (an exact no-op prune, see [`SearchContext`]).
fn move_keys(state: &State, ctx: &SearchContext, plan: &SystemPlan) -> Vec<MoveKey> {
    let mut out = Vec::new();
    if state.system().available_stable_points() > 0 || plan.makeshift_comm_relay {
        out.push(MoveKey::RelayFlip);
    }

    let mut hashes: Vec<u64> = plan.planets.keys().copied().collect();
    hashes.sort_unstable();

    for h in hashes {
        let pre_colonized = state
            .system()
            .get_planet_by_hash(h)
            .is_some_and(|p| p.has_colony());
        if !pre_colonized {
            out.push(MoveKey::ColonizeFlip(h));
        }
        if !plan_develops(state, plan, h) {
            continue;
        }
        let pplan = &plan.planets[&h];

        let mut present: Vec<FacilityType> = pplan.facilities.iter().copied().collect();
        present.sort_unstable();
        for ft in present {
            out.push(MoveKey::FacDrop(h, ft));
        }

        for ft in ctx
            .relevant_facilities(h)
            .iter()
            .filter(|ft| !pplan.facilities.contains(ft))
        {
            out.push(MoveKey::FacAdd(h, *ft));
        }

        for (i, toggle) in Toggle::ALL.iter().enumerate() {
            if matches!(toggle, Toggle::Colonize) {
                continue;
            }
            if matches!(toggle, Toggle::InstallItems) && !ctx.allow_install_items {
                continue;
            }
            out.push(MoveKey::ToggleFlip(h, i as u8));
        }
    }
    out
}

/// Apply `key` to `plan`, returning the neighbor plan, or `None` when the move
/// is currently a no-op/illegal (e.g. dropping a facility the plan no longer
/// includes — possible for stale queue entries).
///
/// Move semantics:
/// * `ColonizeFlip` off → on enters with the full plan (a bare colony with no
///   facilities helps nothing); free_port starts off and can be added by a later
///   toggle move. On → off resets the planet to undeveloped entirely.
/// * `FacAdd` pulls in the prerequisite upgrade chain so the permission actually
///   bites (a Megaport permit is inert without its Spaceport).
fn apply_move(
    state: &State,
    ctx: &SearchContext,
    plan: &SystemPlan,
    key: MoveKey,
) -> Option<SystemPlan> {
    match key {
        MoveKey::RelayFlip => {
            if state.system().available_stable_points() > 0 || plan.makeshift_comm_relay {
                let mut trial = plan.clone();
                trial.makeshift_comm_relay = !trial.makeshift_comm_relay;
                Some(trial)
            } else {
                None
            }
        }
        MoveKey::ColonizeFlip(h) => {
            let pre_colonized = state
                .system()
                .get_planet_by_hash(h)
                .is_some_and(|p| p.has_colony());
            if pre_colonized {
                return None;
            }
            let mut trial = plan.clone();
            let tp = trial.planets.get_mut(&h)?;
            *tp = if plan_develops(state, plan, h) {
                PlanetPlan::default()
            } else {
                planet_plan_full(ctx.relevant_facilities(h), true, false)
            };
            Some(trial)
        }
        MoveKey::FacDrop(h, ft) => {
            if !plan_develops(state, plan, h) || !plan.planets.get(&h)?.facilities.contains(&ft) {
                return None;
            }
            let mut trial = plan.clone();
            trial.planets.get_mut(&h)?.facilities.remove(&ft);
            Some(trial)
        }
        MoveKey::FacAdd(h, ft) => {
            if !plan_develops(state, plan, h) || plan.planets.get(&h)?.facilities.contains(&ft) {
                return None;
            }
            let mut trial = plan.clone();
            let tp = trial.planets.get_mut(&h)?;
            tp.facilities.insert(ft);
            for pre in upgrade_predecessors(ft) {
                tp.facilities.insert(*pre);
            }
            Some(trial)
        }
        MoveKey::ToggleFlip(h, i) => {
            if !plan_develops(state, plan, h) {
                return None;
            }
            let toggle = Toggle::ALL[i as usize];
            let mut trial = plan.clone();
            let tp = trial.planets.get_mut(&h)?;
            let cur = toggle.get(tp);
            toggle.set(tp, !cur);
            Some(trial)
        }
    }
}

/// All legal single-move neighbors of `plan`, in canonical order. Kept as the
/// exhaustive-pass primitive (used by the verification pass, the diagnostic
/// climb and the gap probe); the production climb walks the same move set
/// lazily via [`lazy_hill_climb`].
fn one_move_neighbors(state: &State, ctx: &SearchContext, plan: &SystemPlan) -> Vec<SystemPlan> {
    move_keys(state, ctx, plan)
        .into_iter()
        .filter_map(|key| apply_move(state, ctx, plan, key))
        .collect()
}

/// Swap neighbors: drop facility A and add facility B (with B's prerequisite
/// chain) on the same developed planet in a single move. A swap expresses the
/// credit/time tradeoff directly — freeing one build's budget to fund a more
/// valuable one sooner — which two independent single flips cannot cross in one
/// step on the rugged maximize landscape. Deterministic order, like
/// [`one_move_neighbors`].
fn swap_neighbors(state: &State, ctx: &SearchContext, plan: &SystemPlan) -> Vec<SystemPlan> {
    let mut out = Vec::new();
    let mut hashes: Vec<u64> = plan.planets.keys().copied().collect();
    hashes.sort_unstable();

    for h in hashes {
        if !plan_develops(state, plan, h) {
            continue;
        }
        let pplan = &plan.planets[&h];

        let mut present: Vec<FacilityType> = pplan.facilities.iter().copied().collect();
        present.sort_unstable();
        let missing: Vec<FacilityType> = ctx
            .relevant_facilities(h)
            .iter()
            .copied()
            .filter(|ft| !pplan.facilities.contains(ft))
            .collect();

        for drop in &present {
            for add in &missing {
                let mut trial = plan.clone();
                let tp = trial.planets.get_mut(&h).unwrap();
                tp.facilities.remove(drop);
                tp.facilities.insert(*add);
                for pre in upgrade_predecessors(*add) {
                    tp.facilities.insert(*pre);
                }
                out.push(trial);
            }
        }
    }
    out
}

/// Best-improvement Variable-Neighborhood-Descent from `(best_plan, best)`.
///
/// Each pass evaluates the *whole* fresh neighbourhood and applies the single
/// steepest improving move, then restarts — removing the path-dependence of the
/// old ordered first-improvement scan. The neighbourhood is bidirectional
/// (add/drop/toggle either way via [`one_move_neighbors`]), which lets the climb
/// walk back *up* where the budget tradeoff rewards it instead of only pruning
/// the maximal seed downward. When `use_swaps` is set, drop-A+add-B moves are
/// included to cross basins single flips cannot. Bounded by `deadline`.
#[allow(clippy::too_many_arguments)] // mirrors `choose_planet_set`'s threaded search context
fn hill_climb(
    state: &State,
    ctx: &SearchContext,
    objective: &Objective,
    settings: SolverSettings,
    mut best_plan: SystemPlan,
    mut best: PlanScore,
    use_swaps: bool,
    node_budget: u32,
    nodes_searched: &mut u32,
    cache: &PlanCache,
) -> (SystemPlan, PlanScore) {
    loop {
        if *nodes_searched >= node_budget {
            break;
        }

        let mut neighbors = one_move_neighbors(state, ctx, &best_plan);
        if use_swaps {
            neighbors.extend(swap_neighbors(state, ctx, &best_plan));
        }

        let mut best_move: Option<(SystemPlan, PlanScore)> = None;
        for trial in neighbors {
            if *nodes_searched >= node_budget {
                break;
            }
            *nodes_searched += 1;
            let cand = run_plan_cached(state, objective, &trial, settings, cache);
            // Must beat the incumbent, then be the steepest such move this pass.
            if !is_better(objective, &cand, &trial, &best, &best_plan) {
                continue;
            }
            let steeper = best_move
                .as_ref()
                .map_or(true, |(mp, mo)| is_better(objective, &cand, &trial, mo, mp));
            if steeper {
                best_move = Some((trial, cand));
            }
        }

        match best_move {
            Some((plan, score)) => {
                best_plan = plan;
                best = score;
            }
            None => break,
        }
    }

    (best_plan, best)
}

/// Priority-queue entry for [`lazy_hill_climb`]. Orders by: must-evaluate
/// sentinel first, then the rank fields from [`lazy_rank`] (mirroring
/// [`is_better`]), then earlier insertion first — a total, deterministic order.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LazyEntry {
    sentinel: bool,
    feasible: bool,
    primary: f64,
    months_neg: i64,
    size_neg: i64,
    seq_neg: i64,
    version: u32,
    key: MoveKey,
}

impl Eq for LazyEntry {}

impl Ord for LazyEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.sentinel
            .cmp(&other.sentinel)
            .then_with(|| self.feasible.cmp(&other.feasible))
            .then_with(|| self.primary.total_cmp(&other.primary))
            .then_with(|| self.months_neg.cmp(&other.months_neg))
            .then_with(|| self.size_neg.cmp(&other.size_neg))
            .then_with(|| self.seq_neg.cmp(&other.seq_neg))
            .then_with(|| self.key.cmp(&other.key))
    }
}

impl PartialOrd for LazyEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Heap rank of a candidate, consistent with [`is_better`]: feasibility
/// dominates; feasible reach plans rank by fewer months then smaller plan;
/// feasible maximize plans rank by value, then fewer months, then smaller plan;
/// infeasible plans rank by smaller violation.
fn lazy_rank(objective: &Objective, score: &PlanScore, plan_size: usize) -> (bool, f64, i64, i64) {
    if !score.feasible {
        return (false, -score.violation, 0, 0);
    }
    match objective {
        Objective::Reach(_) => (true, -(score.months as f64), 0, -(plan_size as i64)),
        Objective::Maximize { .. } => (
            true,
            score.value,
            -(score.months as i64),
            -(plan_size as i64),
        ),
    }
}

/// One exhaustive best-improvement pass over the current neighborhood: returns
/// the steepest improving move, or `None` when `plan` is a single-move local
/// optimum (or the node budget ran out). This is exactly the pass the classic
/// climb runs in a loop; the lazy climb runs it only to *verify* convergence.
#[allow(clippy::too_many_arguments)]
fn steepest_improving_move(
    state: &State,
    ctx: &SearchContext,
    objective: &Objective,
    settings: SolverSettings,
    plan: &SystemPlan,
    score: &PlanScore,
    node_budget: u32,
    nodes_searched: &mut u32,
    cache: &PlanCache,
) -> Option<(SystemPlan, PlanScore)> {
    let mut best_move: Option<(SystemPlan, PlanScore)> = None;
    for key in move_keys(state, ctx, plan) {
        if *nodes_searched >= node_budget {
            break;
        }
        let Some(trial) = apply_move(state, ctx, plan, key) else {
            continue;
        };
        *nodes_searched += 1;
        let cand = run_plan_cached(state, objective, &trial, settings, cache);
        if !is_better(objective, &cand, &trial, score, plan) {
            continue;
        }
        let steeper = best_move
            .as_ref()
            .map_or(true, |(mp, mo)| is_better(objective, &cand, &trial, mo, mp));
        if steeper {
            best_move = Some((trial, cand));
        }
    }
    best_move
}

/// Lazy best-improvement descent: the classic climb ([`hill_climb`]) re-simulates
/// the *entire* neighborhood after every accepted move, costing
/// `accepted_moves × neighborhood` simulations per climb. This walk keeps the
/// same move set in a priority queue with possibly-stale scores and a *parking*
/// discipline:
///
/// * Popping a stale (or never-scored sentinel) entry re-evaluates it against
///   the current plan. If it improves the incumbent it re-enters the queue with
///   its fresh rank; otherwise it is **parked** — kept aside, not requeued.
/// * A fresh entry popping therefore always improves (same plan, deterministic
///   score) — accept it immediately. Accepting bumps the version, requeues all
///   parked entries (now stale, to be re-checked lazily) and adds sentinels for
///   moves that newly became legal.
/// * When the queue runs dry, every legal move of the final plan has been
///   evaluated against it and rejected — the plan is a certified single-move
///   local optimum, the exact guarantee the classic climb's last full pass
///   gives, without an extra verification sweep.
///
/// Cost per accepted move is the handful of stale refreshes that outrank it,
/// not a whole neighborhood pass. Determinism: the queue order is total
/// (insertion sequence breaks ties) and the node budget cuts off identically
/// on every run — *unless* the wall-clock deadline or a cooperative cancel
/// fires first ([`should_stop`]), which returns the best-so-far plan and is
/// inherently machine-dependent. Within the time limit the result is still
/// deterministic.
#[allow(clippy::too_many_arguments)]
fn lazy_hill_climb(
    state: &State,
    ctx: &SearchContext,
    objective: &Objective,
    settings: SolverSettings,
    mut best_plan: SystemPlan,
    mut best: PlanScore,
    node_budget: u32,
    nodes_searched: &mut u32,
    cache: &PlanCache,
    start: Instant,
    deadline: Duration,
) -> (SystemPlan, PlanScore) {
    use std::collections::BinaryHeap;
    use std::collections::HashSet as StdHashSet;

    let mut heap: BinaryHeap<LazyEntry> = BinaryHeap::new();
    let mut parked: Vec<LazyEntry> = Vec::new();
    // Keys currently tracked (in the heap or parked); prevents duplicate pushes.
    let mut known: StdHashSet<MoveKey> = StdHashSet::new();
    let mut seq: i64 = 0;
    let mut version: u32 = 0;
    let mut verification_sweeps: u32 = 0;
    let mut accepts: u32 = 0;

    let mut push_sentinel =
        |heap: &mut BinaryHeap<LazyEntry>, seq: &mut i64, version: u32, key: MoveKey| {
            heap.push(LazyEntry {
                sentinel: true,
                feasible: false,
                primary: 0.0,
                months_neg: 0,
                size_neg: 0,
                seq_neg: -*seq,
                version,
                key,
            });
            *seq += 1;
        };

    for key in move_keys(state, ctx, &best_plan) {
        if known.insert(key) {
            push_sentinel(&mut heap, &mut seq, version, key);
        }
    }

    loop {
        if *nodes_searched >= node_budget || should_stop(start, deadline) {
            return (best_plan, best);
        }

        let Some(entry) = heap.pop() else {
            if verification_sweeps >= 20 {
                return (best_plan, best);
            }
            verification_sweeps += 1;

            let old_parked = std::mem::take(&mut parked);
            let mut next_parked = Vec::with_capacity(old_parked.len());
            let mut found_improver = false;
            for entry in old_parked {
                if *nodes_searched >= node_budget || should_stop(start, deadline) {
                    next_parked.push(entry);
                    parked = next_parked;
                    return (best_plan, best);
                }

                let Some(trial) = apply_move(state, ctx, &best_plan, entry.key) else {
                    known.remove(&entry.key);
                    continue;
                };

                *nodes_searched += 1;
                let cand = run_plan_cached(state, objective, &trial, settings, cache);
                let (feasible, primary, months_neg, size_neg) =
                    lazy_rank(objective, &cand, trial.size());
                let refreshed = LazyEntry {
                    sentinel: false,
                    feasible,
                    primary,
                    months_neg,
                    size_neg,
                    seq_neg: -seq,
                    version,
                    key: entry.key,
                };
                seq += 1;

                if is_better(objective, &cand, &trial, &best, &best_plan) {
                    heap.push(refreshed);
                    found_improver = true;
                } else {
                    next_parked.push(refreshed);
                }
            }
            parked = next_parked;

            if found_improver {
                continue;
            }
            return (best_plan, best);
        };

        let Some(trial) = apply_move(state, ctx, &best_plan, entry.key) else {
            // No longer a legal/meaningful move for the current plan.
            known.remove(&entry.key);
            continue;
        };

        if entry.sentinel || entry.version != version {
            // Stale or never scored: evaluate against the current plan.
            *nodes_searched += 1;
            let cand = run_plan_cached(state, objective, &trial, settings, cache);
            let (feasible, primary, months_neg, size_neg) =
                lazy_rank(objective, &cand, trial.size());
            let refreshed = LazyEntry {
                sentinel: false,
                feasible,
                primary,
                months_neg,
                size_neg,
                seq_neg: -seq,
                version,
                key: entry.key,
            };
            seq += 1;
            if is_better(objective, &cand, &trial, &best, &best_plan) {
                heap.push(refreshed);
            } else {
                parked.push(refreshed);
            }
            continue;
        }

        // Fresh entry: scored against the current plan and pushed only if
        // improving, so accept it. (Deterministic simulation: the cached score
        // it was ranked with is the score it still has.)
        let cand = run_plan_cached(state, objective, &trial, settings, cache);
        best_plan = trial;
        best = cand;
        version += 1;
        accepts += 1;
        verification_sweeps = 0;

        // The accepted key flips meaning (e.g. a toggle's reverse); untrack it
        // so the sentinel scan below can requeue it for the new plan.
        known.remove(&entry.key);
        let mut still_parked = Vec::with_capacity(parked.len());
        // Drain cadence: every-2nd was tried (2026-06) — it shifted which tight
        // floors win (+2.4% Tyle st10, −1.2% st9) at 1.5x cost; the differences
        // are local-optimum churn, not systematic quality. Every-4th keeps the
        // speed; SYSTEM_SOLVER_QUALITY=1 drains on every accept for reference runs.
        let full_refresh = quality_mode() || accepts % 4 == 0;
        for e in parked.drain(..) {
            if full_refresh || wake_after_accept(entry.key, e.key) {
                heap.push(e); // version < current ⇒ re-checked lazily
            } else {
                still_parked.push(e);
            }
        }
        parked = still_parked;
        for key in move_keys(state, ctx, &best_plan) {
            if known.insert(key) {
                push_sentinel(&mut heap, &mut seq, version, key);
            }
        }
    }
}

// ===========================================================================
// Evolutionary plan search (experimental, gated by `SYSTEM_SOLVER_EA`)
// ===========================================================================
//
// An alternative Level-1 driver: instead of climbing a handful of ranked seeds
// independently, evolve a *population* of `SystemPlan`s with per-planet uniform
// crossover. A `SystemPlan` is already a natural genome — one `PlanetPlan`
// (7 toggles + a facility set) per planet — and crossover recombines
// independently-good per-planet sub-plans, which restart-from-subset +
// single-move VND structurally cannot do in one step on the rugged maximize
// landscape (see `MAXIMIZE_LOCAL_MINIMA.md`).
//
// Everything else is reused verbatim: `run_plan_cached` for fitness (shared
// memo, parallel evaluation), `is_better` for selection ordering (handles the
// infeasible→feasible gradient), `move_keys`/`apply_move` for legal mutations,
// and `lazy_hill_climb` for the memetic polish (global exploration by the GA,
// local exploitation by the existing climb).
//
// Shape (per solve):
//   1. Seed the population from the planet-set seeds + mutated/random fill.
//   2. Evolve (elitism + tournament + per-planet crossover + move-mutation,
//      with random immigrants on a stall) for `1 - polish_fraction` of the
//      wall-clock.
//   3. Polish: deep-climb the best evolved individuals *and* the best original
//      seeds. Climbing the seeds guarantees at least the default path's
//      ranked-seed climbs, so the EA never loses the precise template builds the
//      tightest floors need.
// `polish_fraction` and the seed-climb count adapt to the system's seed count.
//
// Benchmarked (5-system `system_benchmark.db`, 5 s/point, vs the default climb):
// +39% mean income, 41/51 points improved, 2 small regressions, 0 points lost,
// and 4 feasible frontier points the default climb missed entirely. The win
// comes from the multi-planet systems where the default climb traps in local
// optima (Beta Bhel +178%, Tyle +36%, Westernesse +24%).
//
// Determinism caveat: a seeded SplitMix64 PRNG makes the GA reproducible *given
// the same generation count*, but the GA/polish boundary is wall-clock-based, so
// — unlike the node-budget-bounded default climb — the EA's result is not
// machine-independent even within the deadline. This is acceptable for the
// maximize/ranking use it targets (already wall-clock-bound in practice); keep
// the default climb where bit-reproducibility matters (e.g. reach mode).

fn ea_enabled() -> bool {
    static EA: OnceLock<bool> = OnceLock::new();
    *EA.get_or_init(|| std::env::var_os("SYSTEM_SOLVER_EA").is_some_and(|v| !v.is_empty()))
}

/// Whether the EA evaluates its population/offspring with the cheap `LateApply`
/// surrogate placement (confirming finalists with the accurate placement). Lets
/// the GA run more generations per wall-clock second. Toggleable for A/B:
/// `SYSTEM_SOLVER_EA_SURROGATE=0` forces it off, any other non-empty value on.
fn ea_surrogate_enabled() -> bool {
    static SUR: OnceLock<bool> = OnceLock::new();
    *SUR.get_or_init(|| {
        std::env::var_os("SYSTEM_SOLVER_EA_SURROGATE").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

/// Whether the factored lookahead recombines candidate inputs incrementally
/// (`total - old + new`, O(1)/candidate) vs re-summing all planets (O(planets)).
/// On by default; `SYSTEM_SOLVER_NO_INCSUM=1` forces the O(planets) path so the
/// speedup can be A/B'd (the two are result-identical after `score_units`).
fn incremental_sum_enabled() -> bool {
    static INC: OnceLock<bool> = OnceLock::new();
    *INC.get_or_init(|| std::env::var_os("SYSTEM_SOLVER_NO_INCSUM").is_none())
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_usize_at_least(key: &str, default: usize, min: usize) -> usize {
    env_usize(key, default).max(min)
}

fn env_usize_opt(key: &str) -> Option<usize> {
    std::env::var(key).ok().and_then(|v| v.parse().ok())
}

fn env_f64_opt(key: &str) -> Option<f64> {
    std::env::var(key).ok().and_then(|v| v.parse().ok())
}

/// Deterministic SplitMix64 — small, fast, no external `rand` dependency. The
/// stream is reproducible given the same draw sequence; see the section header
/// for the wall-clock caveat on the EA's overall determinism.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (n must be > 0).
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    fn flip(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }

    /// Uniform in `[0, 1)`.
    fn frac(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// EA hyperparameters with per-knob env overrides for tuning without
/// recompiling. Read once per process (env is constant), like [`ea_enabled`].
#[derive(Clone, Copy, Debug)]
struct EaParams {
    population: usize,
    elite: usize,
    tournament: usize,
    /// Max single moves applied when mutating an offspring (1..=this).
    mutation_moves: usize,
    /// Probability an offspring is mutated after crossover.
    mutation_rate: f64,
    /// Generations of no incumbent improvement before injecting random
    /// immigrants (diversity restart) — *not* a termination, unlike the old
    /// stall stop, which cut exploration short on tight floors.
    stall_generations: u32,
    /// Fraction of the wall-clock budget reserved for the memetic polish phase.
    /// The GA explores in the remaining `1 - polish_fraction`; the polish then
    /// deep-climbs the best individuals and seeds with the rest. `None` ⇒
    /// adaptive: scaled down for many-planet systems (where GA exploration is
    /// the win) and up for small ones (where a baseline-style climb is hard to
    /// beat). Giving the GA the *bulk* of the budget is load-bearing — running
    /// the deep climbs first instead starves the GA and erases the wins.
    polish_fraction: Option<f64>,
    /// Top-N evolved individuals deep-climbed in the polish.
    polish_elites: usize,
    /// Top-N *original* planet-set seeds also deep-climbed in the polish, so the
    /// EA always does at least the default path's ranked-seed climbs — the
    /// precise builds the tightest floors need can't be bred away. `None` ⇒
    /// adaptive: up to 8 of the best seeds (all of them on small systems).
    seed_climbs: Option<usize>,
    /// Per-climb node cap for the polish climbs. Sized to let a climb fully
    /// converge (~20k nodes per the climb path); the wall-clock deadline is the
    /// real governor.
    climb_node_cap: u32,
}

impl EaParams {
    fn get() -> Self {
        static PARAMS: OnceLock<EaParams> = OnceLock::new();
        *PARAMS.get_or_init(|| EaParams {
            population: env_usize_at_least("SYSTEM_SOLVER_EA_POP", 56, 1),
            elite: env_usize_at_least("SYSTEM_SOLVER_EA_ELITE", 6, 1),
            tournament: env_usize_at_least("SYSTEM_SOLVER_EA_TOUR", 3, 1),
            mutation_moves: env_usize_at_least("SYSTEM_SOLVER_EA_MUT", 2, 1),
            mutation_rate: 0.9,
            stall_generations: env_usize("SYSTEM_SOLVER_EA_STALL", 6) as u32,
            polish_fraction: env_f64_opt("SYSTEM_SOLVER_EA_POLISH_FRAC"),
            polish_elites: env_usize_at_least("SYSTEM_SOLVER_EA_POLISH", 4, 1),
            seed_climbs: env_usize_opt("SYSTEM_SOLVER_EA_SEED_CLIMBS"),
            climb_node_cap: env_usize_at_least("SYSTEM_SOLVER_EA_CLIMB_NODES", 60_000, 1) as u32,
        })
    }
}

/// A random plan over the system's planets: each optional planet is colonized by
/// a coin flip, each developed planet gets random toggles and a random subset of
/// its statically-buildable facilities (with prerequisite chains pulled in so no
/// gene is inert).
fn ea_random_plan(state: &State, ctx: &SearchContext, rng: &mut Rng) -> SystemPlan {
    let mut planets = HashMap::new();
    for (hash, planet) in state.system().planets() {
        let pre_colonized = planet.has_colony();
        let colonize = pre_colonized || rng.flip();
        let pplan = if colonize {
            let mut facilities: HashSet<FacilityType> = ctx
                .relevant_facilities(*hash)
                .iter()
                .copied()
                .filter(|_| rng.flip())
                .collect();
            for ft in facilities.iter().copied().collect::<Vec<_>>() {
                for pre in upgrade_predecessors(ft) {
                    facilities.insert(*pre);
                }
            }
            PlanetPlan {
                colonize,
                free_port: rng.flip(),
                hazard_pay: rng.flip(),
                upgrade_admin: rng.flip(),
                improvements: rng.flip(),
                alpha_cores: rng.flip(),
                install_items: ctx.allow_install_items && rng.flip(),
                facilities,
            }
        } else {
            PlanetPlan::default()
        };
        planets.insert(*hash, pplan);
    }
    SystemPlan {
        planets,
        makeshift_comm_relay: state.system().can_build_makeshift_comm_relay() && rng.flip(),
    }
}

/// Per-planet uniform crossover: for each planet the child inherits one parent's
/// whole `PlanetPlan`, and the relay flag from one parent. Recombining whole
/// per-planet sub-plans is the EA's edge over single-move local search.
fn ea_crossover(a: &SystemPlan, b: &SystemPlan, rng: &mut Rng) -> SystemPlan {
    let mut planets = HashMap::with_capacity(a.planets.len());
    for (hash, ap) in &a.planets {
        let chosen = if rng.flip() {
            ap
        } else {
            b.planets.get(hash).unwrap_or(ap)
        };
        planets.insert(*hash, chosen.clone());
    }
    SystemPlan {
        planets,
        makeshift_comm_relay: if rng.flip() {
            a.makeshift_comm_relay
        } else {
            b.makeshift_comm_relay
        },
    }
}

/// Apply 1..=`max_moves` random legal single moves (the same `MoveKey` set the
/// climb walks), so mutations stay legal and meaningful by construction.
fn ea_mutate(
    state: &State,
    ctx: &SearchContext,
    mut plan: SystemPlan,
    rng: &mut Rng,
    max_moves: usize,
) -> SystemPlan {
    let n = 1 + rng.below(max_moves.max(1));
    for _ in 0..n {
        let keys = move_keys(state, ctx, &plan);
        if keys.is_empty() {
            break;
        }
        let key = keys[rng.below(keys.len())];
        if let Some(next) = apply_move(state, ctx, &plan, key) {
            plan = next;
        }
    }
    plan
}

/// Tournament selection: sample `k` population members, return the best by
/// [`is_better`].
fn ea_tournament<'a>(
    pop: &'a [(SystemPlan, PlanScore)],
    objective: &Objective,
    rng: &mut Rng,
    k: usize,
) -> &'a (SystemPlan, PlanScore) {
    let mut best = &pop[rng.below(pop.len())];
    for _ in 1..k.max(1) {
        let cand = &pop[rng.below(pop.len())];
        if is_better(objective, &cand.1, &cand.0, &best.1, &best.0) {
            best = cand;
        }
    }
    best
}

/// Sort scored plans best-first, mirroring the [`is_better`] total order. Shared
/// by the default path's seed ranking and the EA's per-generation/final sorts.
fn sort_plans_by_score(pop: &mut [(SystemPlan, PlanScore)], objective: &Objective) {
    pop.sort_by(|a, b| {
        if is_better(objective, &a.1, &a.0, &b.1, &b.0) {
            std::cmp::Ordering::Less
        } else if is_better(objective, &b.1, &b.0, &a.1, &a.0) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
}

/// Score a batch of plans through the shared memo, evaluating cache misses in
/// parallel. `State` is `!Sync` (interior mutability caches), so — like the seed
/// scorer — each parallel task gets its own clone; the clone is done only for
/// genuine misses (resolved against the memo up front), since the EA's elites
/// and recombinations frequently reproduce already-scored genomes.
fn ea_score(
    state: &State,
    objective: &Objective,
    settings: SolverSettings,
    cache: &PlanCache,
    plans: &[SystemPlan],
) -> Vec<PlanScore> {
    // Hash outside the lock (each `plan_key` allocs + sorts) so the parallel
    // `run_plan_cached` workers aren't blocked on the cache mutex while we key.
    let placement = settings.facility_upgrade_placement;
    let keys: Vec<CacheKey> = plans.iter().map(|p| (plan_key(p), placement)).collect();
    let mut results: Vec<Option<PlanScore>> = {
        let guard = cache.lock().unwrap();
        keys.iter().map(|k| guard.get(k).cloned()).collect()
    };
    let misses: Vec<(usize, State, &SystemPlan)> = plans
        .iter()
        .enumerate()
        .filter(|(i, _)| results[*i].is_none())
        .map(|(i, p)| (i, state.clone(), p))
        .collect();
    let scored: Vec<(usize, PlanScore)> = misses
        .into_par_iter()
        .map(|(i, st, p)| (i, run_plan_cached(&st, objective, p, settings, cache)))
        .collect();
    for (i, s) in scored {
        results[i] = Some(s);
    }
    results
        .into_iter()
        .map(|s| s.expect("every plan scored"))
        .collect()
}

/// Deep-climb a set of candidates in parallel via [`lazy_hill_climb`]. Each
/// climb gets its own `State` clone (`!Sync`), the same per-climb node cap, and
/// the shared wall-clock deadline. Returns the climbed candidates and the total
/// nodes spent.
#[allow(clippy::too_many_arguments)]
fn ea_climb_set(
    state: &State,
    ctx: &SearchContext,
    objective: &Objective,
    settings: SolverSettings,
    cache: &PlanCache,
    candidates: Vec<(SystemPlan, PlanScore)>,
    per_climb_nodes: u32,
    start: Instant,
    deadline: Duration,
) -> (Vec<(SystemPlan, PlanScore)>, u32) {
    if candidates.is_empty() {
        return (Vec::new(), 0);
    }
    // Pre-clone the start state per climb (sequentially: `State` is `!Sync`).
    let work: Vec<(State, SystemPlan, PlanScore)> = candidates
        .into_iter()
        .map(|(p, s)| (state.clone(), p, s))
        .collect();
    let climbed: Vec<((SystemPlan, PlanScore), u32)> = work
        .into_par_iter()
        .map(|(st, plan, score)| {
            let mut n = 0u32;
            let (p, s) = lazy_hill_climb(
                &st,
                ctx,
                objective,
                settings,
                plan,
                score,
                per_climb_nodes,
                &mut n,
                cache,
                start,
                deadline,
            );
            ((p, s), n)
        })
        .collect();
    let nodes: u32 = climbed.iter().map(|(_, n)| n).sum();
    let out = climbed.into_iter().map(|(ps, _)| ps).collect();
    (out, nodes)
}

/// Evolve a population of plans from `seeds`, then memetically polish the top
/// elites with [`lazy_hill_climb`]. Returns the best plan, its score, and the
/// number of plan evaluations spent (cache hits included, mirroring the climb
/// path's node accounting).
#[allow(clippy::too_many_arguments)]
fn ea_search(
    state: &State,
    ctx: &SearchContext,
    objective: &Objective,
    settings: SolverSettings,
    seeds: Vec<SystemPlan>,
    cache: &PlanCache,
    params: EaParams,
    start: Instant,
    deadline: Duration,
) -> Option<(SystemPlan, PlanScore, u32)> {
    let mut rng = Rng::new(0x5EED_C0DE_1234_9ABC ^ seeds.len() as u64);
    let mut nodes: u32 = 0;

    // Surrogate-accelerated evaluation. Score the GA population/offspring with the
    // cheap `LateApply` placement (~1.9x more plan evals per second), so the GA
    // explores more generations; then confirm the finalists with the accurate
    // `ReplayAsap` placement before any winner is chosen, so a `LateApply`
    // mis-estimate can never decide the result. Only meaningful when the accurate
    // placement is the expensive two-pass and upgrades are enabled; otherwise
    // `cheap_settings == settings` and the path is identical to the plain EA.
    let surrogate = ea_surrogate_enabled()
        && settings.facility_upgrade_placement == FacilityUpgradePlacement::ReplayAsap
        && settings.include_industry_upgrades;
    let cheap_settings = if surrogate {
        settings.with_facility_upgrade_placement(FacilityUpgradePlacement::LateApply)
    } else {
        settings
    };

    // Retain the deduped original planet-set seeds: they seed the population
    // *and* (the best of them) get a guaranteed deep climb in the polish, so the
    // EA never loses the default path's ranked-seed climbs.
    let mut seed_pool: Vec<SystemPlan> = Vec::new();
    let mut seed_seen: HashSet<PlanKey> = HashSet::new();
    for seed in seeds {
        if seed_seen.insert(plan_key(&seed)) {
            seed_pool.push(seed);
        }
    }
    if seed_pool.is_empty() {
        return None;
    }

    // Initial population: the seeds first (diverse basins), then mutated seed
    // variants and random plans to fill, deduped by canonical plan key so no
    // slot is spent on a known genome.
    let mut seen: HashSet<PlanKey> = HashSet::new();
    let mut pop_plans: Vec<SystemPlan> = Vec::new();
    for seed in &seed_pool {
        if pop_plans.len() >= params.population {
            break;
        }
        if seen.insert(plan_key(seed)) {
            pop_plans.push(seed.clone());
        }
    }
    let mut guard = 0usize;
    let fill_cap = params.population.saturating_mul(8).max(8);
    while pop_plans.len() < params.population && guard < fill_cap {
        guard += 1;
        let plan = if !pop_plans.is_empty() && rng.flip() {
            let base = pop_plans[rng.below(pop_plans.len())].clone();
            ea_mutate(state, ctx, base, &mut rng, params.mutation_moves)
        } else {
            ea_random_plan(state, ctx, &mut rng)
        };
        if seen.insert(plan_key(&plan)) {
            pop_plans.push(plan);
        }
    }

    let scores = ea_score(state, objective, cheap_settings, cache, &pop_plans);
    nodes += pop_plans.len() as u32;
    let mut pop: Vec<(SystemPlan, PlanScore)> = pop_plans.into_iter().zip(scores).collect();
    sort_plans_by_score(&mut pop, objective);

    // Reserve ~40% of the wall-clock for the polish. This is the knee of the
    // GA/polish tradeoff measured over the benchmark (3 runs per point): the GA
    // finds the winning *basin* early so it doesn't need most of the clock, while
    // deeper polish protects the few fragile points whose un-lucky GA result can
    // fall below baseline. Sweeping the reserve, the worst single-point regression
    // vs the default climb was 0.15→−33%, 0.30→−21%, 0.40→−11%, 0.50→−25%, with
    // the mean win essentially flat (~+34%) throughout. 0.40 wins the downside.
    let size = seed_pool.len();
    let polish_fraction = params.polish_fraction.unwrap_or(0.40).clamp(0.0, 0.9);
    // Climb up to 8 of the best seeds in the polish; on a small system that is
    // *all* of them, making the EA's polish a superset of the default path's
    // ranked-seed climbs.
    let seed_climbs = params.seed_climbs.unwrap_or_else(|| size.min(8));

    // The GA explores until the polish reserve; the polish then exploits with
    // the remaining wall-clock. The GA *must* get the bulk of the budget — an
    // experiment running the deep climbs first (to guarantee a baseline floor)
    // starved the GA and collapsed the win from +36% to ~0%.
    let ga_deadline = Duration::from_secs_f64(deadline.as_secs_f64() * (1.0 - polish_fraction));

    let mut generations = 0u32;
    let mut stagnation = 0u32;
    while !should_stop(start, ga_deadline) {
        generations += 1;
        let target = params.population.saturating_sub(params.elite).max(1);
        // Elites carried forward; track their keys so offspring don't duplicate
        // a survivor (a duplicate would waste an evaluation).
        let mut child_seen: HashSet<PlanKey> = pop
            .iter()
            .take(params.elite)
            .map(|(p, _)| plan_key(p))
            .collect();
        let mut children: Vec<SystemPlan> = Vec::with_capacity(target);

        // On a prolonged stall, spend part of the brood on fresh random
        // immigrants to escape a converged population (diversity restart)
        // instead of terminating early.
        let immigrants = if stagnation >= params.stall_generations {
            target / 4
        } else {
            0
        };

        let mut attempts = 0usize;
        let attempt_cap = target.saturating_mul(6).max(6);
        while children.len() < target && attempts < attempt_cap {
            attempts += 1;
            let child = if children.len() < immigrants {
                ea_random_plan(state, ctx, &mut rng)
            } else {
                let pa = ea_tournament(&pop, objective, &mut rng, params.tournament)
                    .0
                    .clone();
                let pb = ea_tournament(&pop, objective, &mut rng, params.tournament)
                    .0
                    .clone();
                let mut c = ea_crossover(&pa, &pb, &mut rng);
                if rng.frac() < params.mutation_rate {
                    c = ea_mutate(state, ctx, c, &mut rng, params.mutation_moves);
                }
                c
            };
            if child_seen.insert(plan_key(&child)) {
                children.push(child);
            }
        }
        // If dedup starved the brood (small / converged populations), inject
        // fresh random blood rather than shrinking the generation.
        let mut random_guard = 0usize;
        while children.len() < target && random_guard < attempt_cap {
            random_guard += 1;
            let child = ea_random_plan(state, ctx, &mut rng);
            if child_seen.insert(plan_key(&child)) {
                children.push(child);
            }
        }
        if children.is_empty() {
            break;
        }

        let child_scores = ea_score(state, objective, cheap_settings, cache, &children);
        nodes += children.len() as u32;

        let prev_best_key = plan_key(&pop[0].0);
        let mut next: Vec<(SystemPlan, PlanScore)> = pop.into_iter().take(params.elite).collect();
        next.extend(children.into_iter().zip(child_scores));
        sort_plans_by_score(&mut next, objective);
        next.truncate(params.population);
        pop = next;

        if plan_key(&pop[0].0) == prev_best_key {
            stagnation += 1;
        } else {
            stagnation = 0;
        }
    }

    if std::env::var_os("SYSTEM_SOLVER_EA_TRACE").is_some() {
        eprintln!(
            "EA size={size} gens={generations} nodes={nodes} polish_frac={polish_fraction:.2} seed_climbs={seed_climbs}"
        );
    }

    // Memetic polish: deep-climb the GA's best evolved individuals *and* the
    // best original seeds (the latter guarantees at least the default path's
    // ranked-seed climbs — the precise builds the tightest floors need). Each
    // climb runs in parallel to the full deadline.
    //
    // WHICH plans to polish is chosen by the GA's ranking (cheap under the
    // surrogate); the climbs and every downstream comparison run on the accurate
    // `settings`, so a `LateApply` mis-estimate can never decide the winner.
    let mut polish_keys: HashSet<PlanKey> = HashSet::new();
    let mut polish_plans: Vec<SystemPlan> = Vec::new();
    for (plan, _) in pop.iter().take(params.polish_elites) {
        if polish_keys.insert(plan_key(plan)) {
            polish_plans.push(plan.clone());
        }
    }
    if seed_climbs > 0 {
        let scores = ea_score(state, objective, settings, cache, &seed_pool);
        let mut ranked_seeds: Vec<(SystemPlan, PlanScore)> =
            seed_pool.iter().cloned().zip(scores).collect();
        sort_plans_by_score(&mut ranked_seeds, objective);
        for (plan, _) in ranked_seeds.into_iter().take(seed_climbs) {
            if polish_keys.insert(plan_key(&plan)) {
                polish_plans.push(plan);
            }
        }
    }
    // Re-score the polish set with the accurate placement so each climb starts
    // from a true incumbent (a no-op cache hit when not surrogate-evaluating).
    let polish_scores = ea_score(state, objective, settings, cache, &polish_plans);
    let polish_set: Vec<(SystemPlan, PlanScore)> =
        polish_plans.into_iter().zip(polish_scores).collect();

    let mut finalists: Vec<(SystemPlan, PlanScore)> = Vec::new();
    if !polish_set.is_empty() && !should_stop(start, deadline) {
        let (climbed, used) = ea_climb_set(
            state,
            ctx,
            objective,
            settings,
            cache,
            polish_set,
            params.climb_node_cap,
            start,
            deadline,
        );
        nodes += used;
        finalists.extend(climbed);
    } else {
        finalists.extend(polish_set);
    }

    // Confirm the strongest GA individuals under the accurate placement too, so a
    // genuinely-best surrogate-found plan that was not selected for a climb can
    // still win. Cache hits when not surrogate-evaluating, so this is free there.
    let confirm_n = params.elite.max(params.polish_elites);
    let confirm_plans: Vec<SystemPlan> =
        pop.iter().take(confirm_n).map(|(p, _)| p.clone()).collect();
    let confirm_scores = ea_score(state, objective, settings, cache, &confirm_plans);
    finalists.extend(confirm_plans.into_iter().zip(confirm_scores));

    sort_plans_by_score(&mut finalists, objective);
    let best = finalists.into_iter().next()?;
    Some((best.0, best.1, nodes))
}

/// Level 1 (outer): two-level decomposition solve over the given state's planets
/// on a single shared timeline, optimizing `objective`.
///
/// First [`planet_set_seed_plans`] generates planet-set basins (a maximal
/// "colonize everything" plan is often infeasible for a system-wide goal). Then
/// each basin is hill-climbed independently, and the best finished basin wins.
///
/// **Determinism:** the climbs are bounded by a fixed [`MAX_NODES_PER_SEED`] node
/// budget; while the search finishes inside `time_limit` the result is
/// identical on every machine and run. `time_limit` (ms) is additionally a
/// *hard* wall-clock deadline: climbs poll it (and the cooperative cancel
/// flag, [`crate::solver::cancel`]) at node granularity and return their
/// best-so-far plan when it fires, so a solve never runs meaningfully past
/// its budget — at the cost of machine-dependent results in the cutoff case
/// (reported via `cutoff_occurred`). Returns the best plan found, or `None`
/// if no refined seed satisfies the objective's floors.
fn decomp_search_objective(
    initial_state: &mut State,
    objective: &Objective,
    time_limit: u32,
    settings: SolverSettings,
    extra_seeds: &[SystemPlan],
    profile: SearchProfile,
) -> (Option<AStarSearchResult>, Option<SystemPlan>) {
    let start = Instant::now();
    let deadline = Duration::from_millis(time_limit as u64);

    let ctx = SearchContext::new(initial_state);
    let mut seed_plans = if profile.warm_seeds_only && !extra_seeds.is_empty() {
        Vec::new()
    } else {
        planet_set_seed_plans(initial_state, &ctx, objective, start, deadline)
    };
    seed_plans.extend(extra_seeds.iter().cloned());
    if seed_plans.is_empty() {
        return (None, None);
    }
    stats::SEEDS.fetch_add(
        seed_plans.len() as u64,
        std::sync::atomic::Ordering::Relaxed,
    );

    // Evolutionary driver: evolve a population from the seeds instead of
    // climbing the top-ranked few independently. Same fitness, ordering,
    // mutation operators and memetic polish (`lazy_hill_climb`) as the default
    // path. Engages for the full-profile maximize search when the settings
    // request it (the production `--solve`/`--maximize`/Pareto path) or when the
    // `SYSTEM_SOLVER_EA` env override opts in; reach mode and the reduced
    // ranking profiles always climb (determinism / ranking speed).
    let use_evolutionary = objective.metric().is_some()
        && profile.permits_evolutionary()
        && (ea_enabled() || settings.plan_search == PlanSearch::Evolutionary);
    if use_evolutionary {
        let cache: PlanCache = Mutex::new(FxHashMap::default());
        let params = EaParams::get();
        let (best_plan, best, nodes) = match ea_search(
            initial_state,
            &ctx,
            objective,
            settings,
            seed_plans,
            &cache,
            params,
            start,
            deadline,
        ) {
            Some(found) => found,
            None => return (None, None),
        };
        let cutoff_occurred = should_stop(start, deadline);
        if !best.feasible {
            return (None, Some(best_plan));
        }
        let result = AStarSearchResult {
            solution: Some(best.log),
            cost: best.months,
            cutoff_occurred,
            nodes_searched: nodes,
            nodes_pruned_by_bound: 0,
        };
        return (Some(result), Some(best_plan));
    }

    // Score every seed once (cheap relative to a climb), rank them, and climb
    // only the most promising few: most seeds converge into the same basins, so
    // climbing all of them re-discovers the same optima at full price. The
    // climb can still leave a seed's planet subset via Colonize moves, so a
    // mis-ranked seed is recoverable. Stable sort on the deterministic
    // generation order keeps the selection reproducible.
    let cache: PlanCache = Mutex::new(FxHashMap::default());
    let base_state = initial_state.clone();
    let warm_keys: HashSet<PlanKey> = extra_seeds.iter().map(plan_key).collect();
    let mut scored: Vec<(SystemPlan, PlanScore)> = {
        let seeds: Vec<_> = seed_plans
            .into_iter()
            .map(|seed| (base_state.clone(), seed))
            .collect();
        seeds
            .into_par_iter()
            .map(|(state, seed)| {
                let score = run_plan_cached(&state, objective, &seed, settings, &cache);
                (seed, score)
            })
            .collect()
    };
    let seed_evals = scored.len() as u32;
    sort_plans_by_score(&mut scored, objective);
    let mut ranked = scored;
    let mut warm_scored = Vec::new();
    ranked.retain(|(plan, score)| {
        if warm_keys.contains(&plan_key(plan)) {
            warm_scored.push((plan.clone(), score.clone()));
        }
        true
    });
    ranked.truncate(profile.top_seed_climbs);
    let mut selected_keys: HashSet<PlanKey> =
        ranked.iter().map(|(plan, _)| plan_key(plan)).collect();
    for (plan, score) in warm_scored {
        if selected_keys.insert(plan_key(&plan)) {
            ranked.push((plan, score));
        }
    }
    let scored = ranked;

    // Best-improvement VND over a bidirectional neighbourhood (add/drop/toggle
    // either way). Swaps are *off*: the maximize-gap diagnostic
    // (`diagnose_maximize_gap`) showed bidirectional single moves already escape
    // the old drop-only trap (Mia Bravos income 277797 → 303737) and that swaps
    // add nothing from that optimum, so they are not worth the quadratic
    // neighbourhood. See `MAXIMIZE_LOCAL_MINIMA.md`.
    let starts: Vec<_> = scored
        .into_iter()
        .map(|(seed, _)| (initial_state.clone(), seed))
        .collect();
    let climbed: Vec<_> = starts
        .into_par_iter()
        .map(|(state, seed)| {
            climb_from_seed(
                &state,
                &ctx,
                objective,
                settings,
                seed,
                profile.max_nodes_per_seed,
                &cache,
                start,
                deadline,
            )
        })
        .collect();

    let nodes_searched = seed_evals + climbed.iter().map(|(_, _, nodes)| *nodes).sum::<u32>();
    // Cutoff: a seed exhausted its node budget (deterministic; pathological
    // only for FULL, routine for the capped QUICK_* profiles), or the hard
    // wall-clock deadline / cooperative cancel stopped the climbs early
    // (machine-dependent best-effort result).
    let cutoff_occurred = should_stop(start, deadline)
        || climbed
            .iter()
            .any(|(_, _, nodes)| *nodes >= profile.max_nodes_per_seed);
    let (best_plan, best, _) = match climbed.into_iter().reduce(|best, cand| {
        if is_better(objective, &cand.1, &cand.0, &best.1, &best.0) {
            cand
        } else {
            best
        }
    }) {
        Some(best) => best,
        None => return (None, None),
    };

    // The climb may have started from an infeasible seed and failed to repair it
    // (the goal is genuinely unreachable for any plan it reached). Only report a
    // solution that actually satisfies the goal.
    if !best.feasible {
        return (None, Some(best_plan));
    }

    let result = AStarSearchResult {
        solution: Some(best.log),
        cost: best.months,
        cutoff_occurred,
        nodes_searched,
        nodes_pruned_by_bound: 0,
    };
    (Some(result), Some(best_plan))
}

/// Reach-mode entry: minimize the months needed to satisfy `goal`. Thin wrapper
/// over [`decomp_search_objective`] preserving the original public signature the
/// CLI and test suite call.
pub fn decomp_search(
    initial_state: &mut State,
    goal: &Goal,
    time_limit: u32,
    exclude_upgrades: bool,
) -> Option<AStarSearchResult> {
    decomp_search_with_settings(
        initial_state,
        goal,
        time_limit,
        SolverSettings::legacy(!exclude_upgrades),
    )
}

pub fn decomp_search_with_settings(
    initial_state: &mut State,
    goal: &Goal,
    time_limit: u32,
    settings: SolverSettings,
) -> Option<AStarSearchResult> {
    decomp_search_objective(
        initial_state,
        &Objective::Reach(goal),
        time_limit,
        settings,
        &[],
        SearchProfile::FULL,
    )
    .0
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
    exclude_upgrades: bool,
) -> Option<AStarSearchResult> {
    decomp_search_maximize_with_settings(
        initial_state,
        metric,
        floors,
        horizon_months,
        time_limit,
        SolverSettings::legacy(!exclude_upgrades),
    )
}

pub fn decomp_search_maximize_with_settings(
    initial_state: &mut State,
    metric: Metric,
    floors: &Goal,
    horizon_months: i32,
    time_limit: u32,
    settings: SolverSettings,
) -> Option<AStarSearchResult> {
    decomp_search_maximize_seeded(
        initial_state,
        metric,
        floors,
        horizon_months,
        time_limit,
        settings,
        &[],
        SearchProfile::FULL,
    )
    .0
}

#[allow(clippy::too_many_arguments)] // threaded search context
pub(crate) fn decomp_search_maximize_seeded(
    initial_state: &mut State,
    metric: Metric,
    floors: &Goal,
    horizon_months: i32,
    time_limit: u32,
    settings: SolverSettings,
    extra_seeds: &[SystemPlan],
    profile: SearchProfile,
) -> (Option<AStarSearchResult>, Option<SystemPlan>) {
    let objective = Objective::Maximize {
        metric,
        floors,
        horizon_months,
    };
    decomp_search_objective(
        initial_state,
        &objective,
        time_limit,
        settings,
        extra_seeds,
        profile,
    )
}

/// Joint solve: optimize one combined plan over the whole system on a shared
/// timeline and budget. Returns a single result (wrapped in a `Vec` for
/// drop-in symmetry with the other entry points), or empty if unreachable.
pub fn search_system_decomp(
    initial_state: &mut State,
    goal: &Goal,
    time_limit: u32,
    exclude_upgrades: bool,
) -> Vec<AStarSearchResult> {
    decomp_search(initial_state, goal, time_limit, exclude_upgrades)
        .into_iter()
        .collect()
}

pub fn search_system_decomp_with_settings(
    initial_state: &mut State,
    goal: &Goal,
    time_limit: u32,
    settings: SolverSettings,
) -> Vec<AStarSearchResult> {
    decomp_search_with_settings(initial_state, goal, time_limit, settings)
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
    exclude_upgrades: bool,
) -> Vec<AStarSearchResult> {
    decomp_search_maximize(
        initial_state,
        metric,
        floors,
        horizon_months,
        time_limit,
        exclude_upgrades,
    )
    .into_iter()
    .collect()
}

pub fn search_system_maximize_with_settings(
    initial_state: &mut State,
    metric: Metric,
    floors: &Goal,
    horizon_months: i32,
    time_limit: u32,
    settings: SolverSettings,
) -> Vec<AStarSearchResult> {
    decomp_search_maximize_with_settings(
        initial_state,
        metric,
        floors,
        horizon_months,
        time_limit,
        settings,
    )
    .into_iter()
    .collect()
}

/// Seeded maximize-mode joint solve. Extra plans are scored/ranked with the
/// generated seeds, then the best climbed plan is returned for Pareto warm-start
/// chaining.
#[allow(clippy::too_many_arguments)] // threaded search context
pub(crate) fn search_system_maximize_seeded(
    initial_state: &mut State,
    metric: Metric,
    floors: &Goal,
    horizon_months: i32,
    time_limit: u32,
    settings: SolverSettings,
    extra_seeds: &[SystemPlan],
    profile: SearchProfile,
) -> (Vec<AStarSearchResult>, Option<SystemPlan>) {
    let (result, best_plan) = decomp_search_maximize_seeded(
        initial_state,
        metric,
        floors,
        horizon_months,
        time_limit,
        settings,
        extra_seeds,
        profile,
    );
    (result.into_iter().collect(), best_plan)
}

/// One-shot diagnostic for the maximize local-minimum gap (see
/// `MAXIMIZE_LOCAL_MINIMA.md`). Reports, from the same seed:
/// * the seed value,
/// * the optimum a **single-move** best-improvement VND reaches (no swaps),
/// * whether any single move or any **swap** still improves from that optimum,
/// * the optimum the full **swap-enabled** climb reaches.
///
/// This answers "which move type bridges the gap" before committing to swaps.
/// Returns a human-readable report. Not used by the solver itself.
pub fn diagnose_maximize_gap(
    initial_state: &State,
    metric: Metric,
    floors: &Goal,
    horizon_months: i32,
    exclude_upgrades: bool,
) -> String {
    let settings = SolverSettings::legacy(!exclude_upgrades);
    let objective = Objective::Maximize {
        metric,
        floors,
        horizon_months,
    };
    let start = Instant::now();
    let deadline = Duration::from_secs(30);
    let mut nodes: u32 = 0;
    let cache: PlanCache = Mutex::new(FxHashMap::default());

    let ctx = SearchContext::new(initial_state);
    let Some((seed_plan, seed_score)) = choose_planet_set(
        initial_state,
        &ctx,
        &objective,
        settings,
        start,
        deadline,
        &mut nodes,
    ) else {
        return "diagnose_maximize_gap: no feasible planet-set seed".to_string();
    };
    let seed_val = seed_score.value;

    // Single-move best-improvement VND (bidirectional, no swaps). The diagnostic
    // runs each climb to full convergence (`u32::MAX` budget); `nodes` is shared
    // only to report a combined count.
    let (single_plan, single_score) = hill_climb(
        initial_state,
        &ctx,
        &objective,
        settings,
        seed_plan.clone(),
        seed_score.clone(),
        false,
        u32::MAX,
        &mut nodes,
        &cache,
    );

    // Best single-move and best swap reachable from the single-move optimum.
    let best_of = |plans: Vec<SystemPlan>| -> f64 {
        plans
            .into_iter()
            .map(|p| run_plan(initial_state, &objective, &p, settings))
            .filter(|s| s.feasible)
            .map(|s| s.value)
            .fold(f64::NEG_INFINITY, f64::max)
    };
    let best_single_step = best_of(one_move_neighbors(initial_state, &ctx, &single_plan));
    let best_swap_step = best_of(swap_neighbors(initial_state, &ctx, &single_plan));

    // Full swap-enabled climb from the same seed.
    let (_full_plan, full_score) = hill_climb(
        initial_state,
        &ctx,
        &objective,
        settings,
        seed_plan,
        seed_score,
        true,
        u32::MAX,
        &mut nodes,
        &cache,
    );

    format!(
        "maximize-gap diagnostic ({}, horizon {}):\n\
         \x20 seed value ................... {seed_val:.0}\n\
         \x20 single-move VND optimum ..... {:.0} (months {})\n\
         \x20 best single move from it .... {best_single_step:.0} (delta {:+.0})\n\
         \x20 best swap from it ........... {best_swap_step:.0} (delta {:+.0})\n\
         \x20 full swap-enabled optimum ... {:.0} (months {})\n\
         \x20 nodes evaluated ............. {nodes}",
        metric.as_str(),
        horizon_months,
        single_score.value,
        single_score.months,
        best_single_step - single_score.value,
        best_swap_step - single_score.value,
        full_score.value,
        full_score.months,
    )
}
