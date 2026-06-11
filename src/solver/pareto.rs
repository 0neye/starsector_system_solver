use std::collections::HashMap;

use crate::constants::ColonyItem;
use crate::solver::decomp::{search_system_maximize_seeded, SearchProfile, SystemPlan};
use crate::solver::goal::{Goal, Metric};
use crate::solver::state::{Action, Balance, State};
use crate::system::System;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrontierKind {
    Stability,
    Defense,
}

impl FrontierKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FrontierKind::Stability => "stability",
            FrontierKind::Defense => "defense",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParetoPoint {
    pub kind: FrontierKind,
    pub floor: f64,
    pub income: f64,
    pub stability: f64,
    pub defense: f64,
    pub months: i32,
    pub actions: Vec<Action>,
}

impl ParetoPoint {
    fn frontier_x(&self) -> f64 {
        match self.kind {
            FrontierKind::Stability => self.stability,
            FrontierKind::Defense => self.defense,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParetoSolve {
    pub stability_frontier: Vec<ParetoPoint>,
    pub defense_frontier: Vec<ParetoPoint>,
    pub stability_auc: f64,
    pub defense_auc: f64,
    pub score: f64,
    pub recommendation: Option<ParetoPoint>,
}

pub(crate) const STABILITY_FLOORS: [i32; 6] = [5, 6, 7, 8, 9, 10];
pub(crate) const DEFENSE_FLOORS: [f64; 5] = [0.0, 250.0, 500.0, 750.0, 1000.0];
/// Sparse grids for the quick-ranking sweep. They share the full grids'
/// endpoints, so [`frontier_bounds`] normalizes both sweeps over the same
/// domain and the scores stay comparable.
pub(crate) const QUICK_STABILITY_FLOORS: [i32; 3] = [5, 8, 10];
pub(crate) const QUICK_DEFENSE_FLOORS: [f64; 2] = [0.0, 1000.0];
const SCORE_INCOME_UNIT: f64 = 1_000.0;

pub fn solve_pareto(
    system: &System,
    balance: &Balance,
    horizon: i32,
    time_limit: u32,
) -> ParetoSolve {
    let mut samples = Vec::new();

    let mut stability_warm = None;
    let first_stability = STABILITY_FLOORS[0];
    let floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(first_stability));
    if let Some(point) = measure_point_chained(
        system,
        balance,
        FrontierKind::Stability,
        first_stability as f64,
        &floors,
        horizon,
        time_limit,
        &mut stability_warm,
    ) {
        samples.push(point);
    }
    let defense_initial_warm = stability_warm.clone();

    let stability_system = system.clone();
    let stability_balance = balance.clone();
    let defense_system = system.clone();
    let defense_balance = balance.clone();
    let (mut stability_tail, mut defense_samples) = std::thread::scope(|scope| {
        let stability_handle = scope.spawn(move || {
            let mut stability_warm = stability_warm;
            let mut points = Vec::new();
            for stability in STABILITY_FLOORS.iter().copied().skip(1) {
                let floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(stability));
                if let Some(point) = measure_point_chained(
                    &stability_system,
                    &stability_balance,
                    FrontierKind::Stability,
                    stability as f64,
                    &floors,
                    horizon,
                    time_limit,
                    &mut stability_warm,
                ) {
                    points.push(point);
                }
            }
            points
        });

        let defense_handle = scope.spawn(move || {
            let mut points = Vec::new();
            let mut defense_warm = defense_initial_warm;
            for defense in DEFENSE_FLOORS {
                let floors = Goal::new(f64::NEG_INFINITY, Some(defense), Some(0));
                if let Some(point) = measure_point_chained(
                    &defense_system,
                    &defense_balance,
                    FrontierKind::Defense,
                    defense,
                    &floors,
                    horizon,
                    time_limit,
                    &mut defense_warm,
                ) {
                    points.push(point);
                }
            }
            points
        });

        (
            stability_handle.join().expect("stability chain panicked"),
            defense_handle.join().expect("defense chain panicked"),
        )
    });

    samples.append(&mut stability_tail);
    samples.append(&mut defense_samples);

    assemble_solve(samples)
}

/// Quick-ranking variant of [`solve_pareto`]: a sparse floor grid (3 stability
/// plus 2 defense points instead of 11), a reduced-effort anchor solve, and
/// node-capped repair climbs warm-started along each chain. Deterministic, and
/// shares the frontier/AUC/score code with the full sweep so the two can't
/// drift. Every search budget is a strict reduction of the full sweep's, so
/// each *point's* income is a lower bound on what the full search would find
/// there. The *score* is in practice below the full-sweep score too (benchmark
/// ratios 0.876–0.975), but not provably: the trapezoid AUC over the sparse
/// grid can overestimate the area the full grid would measure between shared
/// floors. See `QUICK_RANKING_DESIGN.md`.
pub fn solve_pareto_quick(
    system: &System,
    balance: &Balance,
    horizon: i32,
    time_limit: u32,
) -> ParetoSolve {
    let mut samples = Vec::new();

    // Anchor: the stability-5 point, searched with full seeding but only the
    // top-2 seed climbs. Its plan warm-starts everything else.
    let mut stability_warm = None;
    let first_stability = QUICK_STABILITY_FLOORS[0];
    let floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(first_stability));
    if let Some(point) = measure_point_seeded(
        system,
        balance,
        FrontierKind::Stability,
        first_stability as f64,
        &floors,
        horizon,
        time_limit,
        &mut stability_warm,
        &[],
        SearchProfile::QUICK,
    ) {
        samples.push(point);
    }
    let mut defense_warm = stability_warm.clone();

    // Repairs: sequential capped climbs from the chain's warm plan. Ranking
    // runs many systems, so there is no per-system chain concurrency here; the
    // rayon pool stays busy inside the anchor solves.
    //
    // The quick chains take bigger floor steps than the full sweep (5→8→10 vs
    // 5→6→…→10), and on large systems a capped repair climb can fail to bridge
    // one — an *infeasible* point. Dropping it would zero out real AUC area
    // (the catastrophic error mode for ranking), so feasibility failures fall
    // back to a fresh reduced-effort search; mere suboptimality does not.
    let mut measure = |kind: FrontierKind,
                       x: f64,
                       floors: &Goal,
                       warm: &mut Option<SystemPlan>|
     -> Option<ParetoPoint> {
        measure_point_seeded(
            system,
            balance,
            kind,
            x,
            floors,
            horizon,
            time_limit,
            warm,
            &[],
            SearchProfile::QUICK_REPAIR,
        )
        .or_else(|| {
            measure_point_seeded(
                system,
                balance,
                kind,
                x,
                floors,
                horizon,
                time_limit,
                warm,
                &[],
                SearchProfile::QUICK,
            )
        })
    };

    for stability in QUICK_STABILITY_FLOORS.iter().copied().skip(1) {
        let floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(stability));
        if let Some(point) = measure(
            FrontierKind::Stability,
            stability as f64,
            &floors,
            &mut stability_warm,
        ) {
            samples.push(point);
        }
    }
    for defense in QUICK_DEFENSE_FLOORS {
        let floors = Goal::new(f64::NEG_INFINITY, Some(defense), Some(0));
        if let Some(point) = measure(FrontierKind::Defense, defense, &floors, &mut defense_warm) {
            samples.push(point);
        }
    }

    assemble_solve(samples)
}

/// Tier-0 "instant paint" variant of [`solve_pareto`]: scores the fixed
/// template portfolio with **no** hill-climb ([`SearchProfile::TEMPLATE`]) over
/// the same sparse floor grid as [`solve_pareto_quick`], chaining each floor's
/// winning template forward as a warm seed. One forced simulation per template,
/// so it paints a rough ranking in milliseconds (the UI shows this immediately
/// and refines to Tier 1 in the background). Shares the frontier/AUC/score code
/// with the full sweep so its scores stay comparable. Because it never climbs,
/// each point's income is in practice a lower bound on the corresponding
/// `solve_pareto_quick` point — refinement only moves scores up. (Not provable:
/// after the first floor the two modes' warm chains diverge, so later floors do
/// not climb from identical seed sets.) See `QUICK_RANKING_DESIGN.md`.
pub fn solve_pareto_template(
    system: &System,
    balance: &Balance,
    horizon: i32,
    time_limit: u32,
) -> ParetoSolve {
    let mut samples = Vec::new();

    // Anchor: the stability-5 template winner, warm-starting everything else.
    let mut stability_warm = None;
    let first_stability = QUICK_STABILITY_FLOORS[0];
    let floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(first_stability));
    if let Some(point) = measure_point_seeded(
        system,
        balance,
        FrontierKind::Stability,
        first_stability as f64,
        &floors,
        horizon,
        time_limit,
        &mut stability_warm,
        &[],
        SearchProfile::TEMPLATE,
    ) {
        samples.push(point);
    }
    let mut defense_warm = stability_warm.clone();

    // Every floor uses the same no-climb template scoring; the warm plan from
    // the previous floor joins the template set so a higher floor can reuse a
    // lower floor's winner verbatim. No QUICK_REPAIR/QUICK fallback here —
    // there is only one (cheapest) profile to fall back to.
    let mut measure = |kind: FrontierKind,
                       x: f64,
                       floors: &Goal,
                       warm: &mut Option<SystemPlan>|
     -> Option<ParetoPoint> {
        measure_point_seeded(
            system,
            balance,
            kind,
            x,
            floors,
            horizon,
            time_limit,
            warm,
            &[],
            SearchProfile::TEMPLATE,
        )
    };

    for stability in QUICK_STABILITY_FLOORS.iter().copied().skip(1) {
        let floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(stability));
        if let Some(point) = measure(
            FrontierKind::Stability,
            stability as f64,
            &floors,
            &mut stability_warm,
        ) {
            samples.push(point);
        }
    }
    for defense in QUICK_DEFENSE_FLOORS {
        let floors = Goal::new(f64::NEG_INFINITY, Some(defense), Some(0));
        if let Some(point) = measure(FrontierKind::Defense, defense, &floors, &mut defense_warm) {
            samples.push(point);
        }
    }

    assemble_solve(samples)
}

/// Tier-0 *upper-bound* variant: the per-planet decomposed, credit-relaxed
/// income ceiling ([`per_planet_income_bound`]), reported as a `ParetoSolve`
/// whose score upper-bounds the full-sweep score. Unlike
/// [`solve_pareto_template`] (which approximates the frontier from below), this
/// is the "potential" ceiling the interval-escalation step wants. Caveat for
/// certificate use: the ceiling is certified only up to the greedy one-shot
/// rationing, which is optimal under concavity but can in principle
/// under-allocate when units complement each other (see
/// [`per_planet_income_bound`]); the benchmark margin is wide (bound/full
/// 1.36–2.02). No frontier search at all — one FULL solve per single-planet
/// sub-system, rationing the shared one-shots. See `QUICK_RANKING_DESIGN.md`
/// step 1.
pub fn solve_pareto_bound(
    system: &System,
    balance: &Balance,
    horizon: i32,
    time_limit: u32,
) -> ParetoSolve {
    let ceiling = per_planet_income_bound(system, balance, horizon, time_limit);

    // Income is non-increasing as either floor tightens, so the unconstrained
    // per-planet ceiling bounds the achievable income at *every* floor. A
    // frontier flat at that ceiling has normalized AUC equal to the ceiling on
    // both axes, so the score it implies — an upper bound on the real AUC score —
    // is `ceiling / SCORE_INCOME_UNIT`. We construct the `ParetoSolve` directly
    // rather than feed a flat frontier through `pareto_frontier` (which would
    // dedup the equal-income points to one and collapse the AUC to zero).
    let (smin, smax) = frontier_bounds(FrontierKind::Stability);
    let (dmin, dmax) = frontier_bounds(FrontierKind::Defense);
    let stability_point = |x: f64| ParetoPoint {
        kind: FrontierKind::Stability,
        floor: x,
        income: ceiling,
        stability: x,
        defense: 0.0,
        months: 0,
        actions: Vec::new(),
    };
    let defense_point = |x: f64| ParetoPoint {
        kind: FrontierKind::Defense,
        floor: x,
        income: ceiling,
        stability: 0.0,
        defense: x,
        months: 0,
        actions: Vec::new(),
    };

    ParetoSolve {
        stability_frontier: vec![stability_point(smin), stability_point(smax)],
        defense_frontier: vec![defense_point(dmin), defense_point(dmax)],
        stability_auc: ceiling,
        defense_auc: ceiling,
        score: ceiling / SCORE_INCOME_UNIT,
        recommendation: None,
    }
}

/// Per-planet decomposed, credit-relaxed **upper bound** on the system's net
/// income — the Tier-0 "potential" ceiling. Two relaxations make it a ceiling
/// that is also cheap to compute:
///
/// * **Drop cross-planet contention.** Net income is a pure sum of per-planet
///   incomes, so solving each planet on its own infinite-credit timeline (no
///   shared credit pool, no shared `Wait`) and summing can only *exceed* the
///   joint optimum, where those resources are contended. The one system-wide
///   coupling — the comm-relay stability bonus — is preserved exactly by
///   [`single_planet_system`], so it is not a source of error.
/// * **Relax credits, keep the one-shot caps real.** Per `bound.rs`, credits are
///   the renewable resource; relaxing them removes the schedule ruggedness so a
///   single-planet FULL search reliably reaches the planet's ceiling. Story
///   points, alpha cores and colony items stay scarce and are rationed across
///   planets by greedy marginal income (each unit to the planet it helps most),
///   so the bound is not the vacuous "a core on every planet" number.
///
/// The result upper-bounds the joint optimum *when the greedy rationing is
/// optimal*, which holds when each planet's income is concave in its one-shot
/// allocation (diminishing returns make that near-true). Under complementarities
/// — a unit whose gain only materializes alongside a second unit — the greedy
/// can under-allocate, so this is a near-certain ceiling rather than a certified
/// one; don't build pruning that assumes it can never undershoot. See
/// `QUICK_RANKING_DESIGN.md` step 1.
fn per_planet_income_bound(
    system: &System,
    balance: &Balance,
    horizon: i32,
    time_limit: u32,
) -> f64 {
    let mut hashes: Vec<u64> = system.planets().keys().copied().collect();
    hashes.sort_unstable();
    let subs: Vec<System> = hashes
        .iter()
        .map(|h| single_planet_system(system, *h))
        .collect();
    let n = subs.len();
    if n == 0 {
        return 0.0;
    }

    // Shared one-shot pool. Item types are held in a fixed sorted order so a
    // planet's item allocation is a count vector aligned to `item_types`.
    let mut item_types: Vec<ColonyItem> = balance.colony_items().keys().copied().collect();
    item_types.sort_unstable();
    let item_totals: Vec<u32> = item_types
        .iter()
        .map(|it| *balance.colony_items().get(it).unwrap_or(&0))
        .collect();
    let n_items = item_types.len();

    // Memoized single-planet solve. After a greedy step only the chosen planet's
    // allocation changes, so every other planet's trials in the next pass hit the
    // memo — the whole sweep costs ~`(planets + units) * resource_kinds` solves.
    let mut memo: HashMap<(usize, u32, u32, Vec<u32>), f64> = HashMap::new();
    let mut solve = |idx: usize, cores: u32, sps: u32, items: &[u32]| -> f64 {
        let key = (idx, cores, sps, items.to_vec());
        if let Some(value) = memo.get(&key) {
            return *value;
        }
        let alloc: Vec<(ColonyItem, u32)> = item_types
            .iter()
            .copied()
            .zip(items.iter().copied())
            .filter(|(_, count)| *count > 0)
            .collect();
        let value = solve_planet_income(&subs[idx], cores, sps, &alloc, horizon, time_limit);
        memo.insert(key, value);
        value
    };

    // Per-planet running allocation and its achieved income.
    let mut cores = vec![0u32; n];
    let mut sps = vec![0u32; n];
    let mut items = vec![vec![0u32; n_items]; n];
    let mut incomes: Vec<f64> = (0..n).map(|p| solve(p, 0, 0, &items[p])).collect();

    let mut cores_rem = balance.alpha_cores();
    let mut sps_rem = balance.story_points();
    let mut items_rem = item_totals;

    // Greedy: assign each remaining one-shot unit to the (planet, resource) with
    // the largest marginal income gain, until the pool is spent or no unit helps.
    #[derive(Clone, Copy)]
    enum Unit {
        Core,
        Sp,
        Item(usize),
    }
    loop {
        let mut best: Option<(f64, usize, Unit)> = None;
        let mut consider = |gain: f64, p: usize, unit: Unit| {
            // 1 credit threshold treats float noise as no gain (incomes are in
            // the hundreds of thousands).
            if gain > 1.0 && best.is_none_or(|(g, _, _)| gain > g) {
                best = Some((gain, p, unit));
            }
        };
        for p in 0..n {
            let cur = incomes[p];
            if cores_rem > 0 {
                consider(
                    solve(p, cores[p] + 1, sps[p], &items[p]) - cur,
                    p,
                    Unit::Core,
                );
            }
            if sps_rem > 0 {
                consider(solve(p, cores[p], sps[p] + 1, &items[p]) - cur, p, Unit::Sp);
            }
            for j in 0..n_items {
                if items_rem[j] > 0 {
                    let mut trial = items[p].clone();
                    trial[j] += 1;
                    consider(solve(p, cores[p], sps[p], &trial) - cur, p, Unit::Item(j));
                }
            }
        }

        match best {
            Some((_, p, Unit::Core)) => {
                cores[p] += 1;
                cores_rem -= 1;
            }
            Some((_, p, Unit::Sp)) => {
                sps[p] += 1;
                sps_rem -= 1;
            }
            Some((_, p, Unit::Item(j))) => {
                items[p][j] += 1;
                items_rem[j] -= 1;
            }
            None => break,
        }
        if let Some((_, p, _)) = best {
            incomes[p] = solve(p, cores[p], sps[p], &items[p]);
        }
    }

    incomes.iter().sum()
}

/// One-planet view of `system`: a clone with every other planet removed, so the
/// kept planet retains the system's stable points and infrastructure — hence its
/// comm-relay status and the system-wide stability bonus that depends on it.
/// Each per-planet solve can build its own relay; because a relay benefits every
/// planet identically in the real joint problem, granting it per planet is exact,
/// not a relaxation.
fn single_planet_system(system: &System, keep: u64) -> System {
    let mut sub = system.clone();
    let drop: Vec<u64> = sub
        .planets()
        .keys()
        .copied()
        .filter(|hash| *hash != keep)
        .collect();
    for hash in drop {
        sub.remove_planet_by_hash(hash);
    }
    sub
}

/// Max net income at the horizon for a single-planet sub-system under relaxed
/// credits and a fixed one-shot allocation. Credits are inflated so nothing is
/// ever credit-gated (the renewable resource, per `bound.rs`); the supplied
/// `cores`/`sps`/`items` pin the scarce one-shots so the caller can ration them
/// across planets. A FULL search converges trivially on one planet, and the
/// credit relaxation removes the schedule ruggedness, so the value is a
/// trustworthy per-planet ceiling.
fn solve_planet_income(
    sub_system: &System,
    cores: u32,
    sps: u32,
    items: &[(ColonyItem, u32)],
    horizon: i32,
    time_limit: u32,
) -> f64 {
    let mut balance = Balance::new(1e15, sps, cores);
    for (item, count) in items {
        for _ in 0..*count {
            balance.add_colony_item(*item);
        }
    }
    let floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(0));
    let mut warm = None;
    measure_point_seeded(
        sub_system,
        &balance,
        FrontierKind::Stability,
        0.0,
        &floors,
        horizon,
        time_limit,
        &mut warm,
        &[],
        SearchProfile::FULL,
    )
    .map_or(0.0, |point| point.income)
}

/// Shared back half of [`solve_pareto`] / [`solve_pareto_quick`]: frontier
/// filtering, AUC scoring and the balanced recommendation. Both modes must go
/// through this so quick scores stay comparable with full scores.
fn assemble_solve(samples: Vec<ParetoPoint>) -> ParetoSolve {
    let stability_frontier = pareto_frontier(
        samples
            .iter()
            .filter(|p| p.kind == FrontierKind::Stability)
            .cloned()
            .collect(),
    );
    let defense_frontier = pareto_frontier(
        samples
            .iter()
            .filter(|p| p.kind == FrontierKind::Defense)
            .cloned()
            .collect(),
    );

    let stability_auc = normalized_auc(&stability_frontier, FrontierKind::Stability);
    let defense_auc = normalized_auc(&defense_frontier, FrontierKind::Defense);
    let score = ((stability_auc + defense_auc) / 2.0) / SCORE_INCOME_UNIT;
    let recommendation = recommend_tradeoff(&stability_frontier, &defense_frontier);

    ParetoSolve {
        stability_frontier,
        defense_frontier,
        stability_auc,
        defense_auc,
        score,
        recommendation,
    }
}

#[allow(clippy::too_many_arguments)] // mirrors measure_point_seeded
pub(crate) fn measure_point_chained(
    system: &System,
    balance: &Balance,
    kind: FrontierKind,
    floor: f64,
    floors: &Goal,
    horizon: i32,
    time_limit: u32,
    warm: &mut Option<SystemPlan>,
) -> Option<ParetoPoint> {
    measure_point_seeded(
        system,
        balance,
        kind,
        floor,
        floors,
        horizon,
        time_limit,
        warm,
        &[],
        SearchProfile::FULL,
    )
}

/// [`measure_point_chained`] plus caller-supplied seed plans beyond the chain's
/// warm plan. The bound sweep cross-seeds each credit-relaxed solve with the
/// same floor's real-budget plan (feasible and no worse under relaxed credits),
/// so `bound >= greedy` holds by construction instead of relying on the relaxed
/// climb to rediscover what the greedy already found.
#[allow(clippy::too_many_arguments)] // mirrors measure_point_chained
pub(crate) fn measure_point_seeded(
    system: &System,
    balance: &Balance,
    kind: FrontierKind,
    floor: f64,
    floors: &Goal,
    horizon: i32,
    time_limit: u32,
    warm: &mut Option<SystemPlan>,
    extra_seeds: &[SystemPlan],
    profile: SearchProfile,
) -> Option<ParetoPoint> {
    let mut state = State::new(balance.clone(), system.clone());
    let base = state.clone();
    let mut seeds: Vec<SystemPlan> = warm.iter().cloned().collect();
    seeds.extend(extra_seeds.iter().cloned());
    let (results, best_plan) = search_system_maximize_seeded(
        &mut state,
        Metric::Income,
        floors,
        horizon,
        time_limit,
        true,
        &seeds,
        profile,
    );
    *warm = best_plan;
    let result = results.into_iter().next()?;
    let actions = result.solution.unwrap_or_default();

    let mut replay = base;
    for action in &actions {
        replay.apply_action_raw(action, false);
    }

    Some(ParetoPoint {
        kind,
        floor,
        income: replay.balance().net_income(),
        stability: replay.system().avg_stability(),
        defense: replay.system().avg_ground_defense(),
        months: result.cost,
        actions,
    })
}

fn pareto_frontier(points: Vec<ParetoPoint>) -> Vec<ParetoPoint> {
    let mut frontier: Vec<ParetoPoint> = points
        .iter()
        .filter(|candidate| {
            !points.iter().any(|other| {
                other.kind == candidate.kind
                    && other.frontier_x() >= candidate.frontier_x()
                    && other.income >= candidate.income
                    && (other.frontier_x() > candidate.frontier_x()
                        || other.income > candidate.income)
            })
        })
        .cloned()
        .collect();

    frontier.sort_by(|a, b| {
        a.frontier_x()
            .total_cmp(&b.frontier_x())
            .then_with(|| b.income.total_cmp(&a.income))
            .then_with(|| a.months.cmp(&b.months))
    });

    // The dominated-filter above leaves at most one income per x (a higher-income
    // same-x point would have dominated the other), so any same-x survivors are
    // exact income ties. The sort already orders the min-months one first, so
    // keep the first of each (kind, x) group.
    frontier.dedup_by(|a, b| a.kind == b.kind && a.frontier_x() == b.frontier_x());

    frontier
}

fn normalized_auc(frontier: &[ParetoPoint], kind: FrontierKind) -> f64 {
    let (min_x, max_x) = frontier_bounds(kind);
    let span = max_x - min_x;
    if span <= 0.0 {
        return 0.0;
    }

    raw_auc(frontier, min_x, max_x) / span
}

fn raw_auc(frontier: &[ParetoPoint], min_x: f64, max_x: f64) -> f64 {
    frontier
        .windows(2)
        .map(|pair| {
            let x0 = pair[0].frontier_x().clamp(min_x, max_x);
            let x1 = pair[1].frontier_x().clamp(min_x, max_x);
            let y0 = pair[0].income.max(0.0);
            let y1 = pair[1].income.max(0.0);
            (x1 - x0).max(0.0) * (y0 + y1) / 2.0
        })
        .sum()
}

fn frontier_bounds(kind: FrontierKind) -> (f64, f64) {
    match kind {
        FrontierKind::Stability => (
            STABILITY_FLOORS[0] as f64,
            STABILITY_FLOORS[STABILITY_FLOORS.len() - 1] as f64,
        ),
        FrontierKind::Defense => (DEFENSE_FLOORS[0], DEFENSE_FLOORS[DEFENSE_FLOORS.len() - 1]),
    }
}

fn recommend_tradeoff(
    stability_frontier: &[ParetoPoint],
    defense_frontier: &[ParetoPoint],
) -> Option<ParetoPoint> {
    let all: Vec<&ParetoPoint> = stability_frontier
        .iter()
        .chain(defense_frontier.iter())
        .collect();

    let max_income = all
        .iter()
        .map(|p| p.income.max(0.0))
        .fold(0.0_f64, f64::max);
    let max_stability = all.iter().map(|p| p.stability).fold(0.0_f64, f64::max);
    let max_defense = all.iter().map(|p| p.defense).fold(0.0_f64, f64::max);

    all.into_iter()
        .max_by(|a, b| {
            let score_a = balanced_point_score(a, max_income, max_stability, max_defense);
            let score_b = balanced_point_score(b, max_income, max_stability, max_defense);
            score_a
                .total_cmp(&score_b)
                .then_with(|| a.income.total_cmp(&b.income))
                .then_with(|| b.months.cmp(&a.months))
        })
        .cloned()
}

fn balanced_point_score(
    point: &ParetoPoint,
    max_income: f64,
    max_stability: f64,
    max_defense: f64,
) -> f64 {
    let income = normalized(point.income.max(0.0), max_income);
    let stability = normalized(point.stability, max_stability);
    let defense = normalized(point.defense, max_defense);

    income * 0.5 + stability * 0.25 + defense * 0.25
}

fn normalized(value: f64, max: f64) -> f64 {
    if max <= 0.0 {
        0.0
    } else {
        value / max
    }
}

#[cfg(test)]
mod tests {
    use super::{normalized_auc, pareto_frontier, raw_auc, FrontierKind, ParetoPoint};

    fn point(x: f64, income: f64) -> ParetoPoint {
        ParetoPoint {
            kind: FrontierKind::Stability,
            floor: x,
            income,
            stability: x,
            defense: 0.0,
            months: 0,
            actions: Vec::new(),
        }
    }

    #[test]
    fn frontier_removes_dominated_points() {
        let frontier = pareto_frontier(vec![point(5.0, 100.0), point(6.0, 90.0), point(5.0, 80.0)]);
        assert_eq!(frontier.len(), 2);
        assert_eq!(frontier[0].stability, 5.0);
        assert_eq!(frontier[0].income, 100.0);
        assert_eq!(frontier[1].stability, 6.0);
        assert_eq!(frontier[1].income, 90.0);
    }

    #[test]
    fn auc_uses_trapezoids() {
        let frontier = vec![point(5.0, 100.0), point(6.0, 50.0), point(8.0, 25.0)];
        assert_eq!(raw_auc(&frontier, 5.0, 10.0), 150.0);
    }

    #[test]
    fn normalized_auc_divides_by_frontier_domain() {
        let frontier = vec![point(5.0, 100.0), point(10.0, 50.0)];
        assert_eq!(normalized_auc(&frontier, FrontierKind::Stability), 75.0);
    }
}
