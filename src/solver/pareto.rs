use std::collections::HashMap;

use crate::constants::{ColonyItem, Resource};
use crate::solver::decomp::{search_system_maximize_seeded, SearchProfile, SystemPlan};
use crate::solver::goal::{Goal, Metric};
use crate::solver::state::{Action, Balance, State};
use crate::solver::SolverSettings;
use crate::system::System;
use rayon::prelude::*;

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
    include_industry_upgrades: bool,
) -> ParetoSolve {
    solve_pareto_with_settings(
        system,
        balance,
        horizon,
        time_limit,
        SolverSettings::legacy(include_industry_upgrades),
    )
}

pub fn solve_pareto_with_settings(
    system: &System,
    balance: &Balance,
    horizon: i32,
    time_limit: u32,
    settings: SolverSettings,
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
        settings,
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
            crate::cpu_affinity::prefer_performance_cores();

            let mut stability_warm = stability_warm;
            let mut points = Vec::new();
            for stability in STABILITY_FLOORS.iter().copied().skip(1) {
                if crate::solver::cancel::is_cancelled() {
                    break;
                }
                let floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(stability));
                if let Some(point) = measure_point_chained(
                    &stability_system,
                    &stability_balance,
                    FrontierKind::Stability,
                    stability as f64,
                    &floors,
                    horizon,
                    time_limit,
                    settings,
                    &mut stability_warm,
                ) {
                    points.push(point);
                }
            }
            points
        });

        let defense_handle = scope.spawn(move || {
            crate::cpu_affinity::prefer_performance_cores();

            let mut points = Vec::new();
            let mut defense_warm = defense_initial_warm;
            for defense in DEFENSE_FLOORS {
                if crate::solver::cancel::is_cancelled() {
                    break;
                }
                let floors = Goal::new(f64::NEG_INFINITY, Some(defense), Some(0));
                if let Some(point) = measure_point_chained(
                    &defense_system,
                    &defense_balance,
                    FrontierKind::Defense,
                    defense,
                    &floors,
                    horizon,
                    time_limit,
                    settings,
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
/// mini-anchor repair solves warm-started along each chain, with a hidden
/// one-step feasibility bridge when a sparse jump fails (see `measure` below).
/// Deterministic, and shares the frontier/AUC/score code with the full sweep so
/// the two can't drift. Every search budget is a strict reduction of the full
/// sweep's, so each *point's* income is a lower bound on what the full search
/// would find there. The *score* is in practice below the full-sweep score too
/// (quality-reference ratios 0.84-0.98 upgrades-on, 0.78-0.99 upgrades-off),
/// but not provably: the trapezoid AUC over the sparse grid can overestimate
/// the area the full grid would measure between shared floors. See
/// `workspace/QUICK_RANKING_DESIGN.md`.
pub fn solve_pareto_quick(
    system: &System,
    balance: &Balance,
    horizon: i32,
    time_limit: u32,
    include_industry_upgrades: bool,
) -> ParetoSolve {
    solve_pareto_quick_with_settings(
        system,
        balance,
        horizon,
        time_limit,
        SolverSettings::legacy(include_industry_upgrades),
    )
}

pub fn solve_pareto_quick_with_settings(
    system: &System,
    balance: &Balance,
    horizon: i32,
    time_limit: u32,
    settings: SolverSettings,
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
        settings,
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
    // The quick chains take bigger floor steps than the full sweep (5->8->10 vs
    // 5->6->...->10, and 0->1000 defense), and on large systems a capped repair
    // climb can fail to bridge one while the target is still feasible. When
    // QUICK_REPAIR and QUICK both miss a target, walk hidden one-step
    // intermediate floors from the previous sparse floor toward the target
    // using the same two profiles, purely to advance the warm plan. Dropping the
    // target still zeroes real AUC area, but if any intermediate step is truly
    // infeasible, monotonicity lets us concede the target too.
    let mut measure_quick = |kind: FrontierKind,
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
            settings,
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
                settings,
                warm,
                &[],
                SearchProfile::QUICK,
            )
        })
    };

    // NOTE (2026-06-12): a cheap-first warm-only repair (climb only the warm
    // plan at a 2k node cap, escalate to the pair below when the income drops
    // more than an acceptance fraction vs the previous chain point) was tried
    // and reverted: at a 0.8 fraction it saved ~13% wall time but visibly
    // eroded per-point incomes (Beta Bhel ratio 0.84 -> 0.80); at 0.9 the
    // escalations dominated and the probe was pure overhead. Big systems are
    // deadline-bound per point, so the protective escalations cost exactly
    // the budget the probe tries to save.
    let mut measure = |kind: FrontierKind,
                       prev_x: f64,
                       x: f64,
                       floors: &Goal,
                       warm: &mut Option<SystemPlan>|
     -> Option<ParetoPoint> {
        let previous_warm = warm.clone();
        if let Some(point) = measure_quick(kind, x, floors, warm) {
            return Some(point);
        }
        *warm = previous_warm;

        let mut intermediate_floors = Vec::new();
        match kind {
            FrontierKind::Stability => {
                let mut stability = prev_x as i32 + 1;
                let target = x as i32;
                while stability < target {
                    intermediate_floors.push(stability as f64);
                    stability += 1;
                }
            }
            FrontierKind::Defense => {
                let mut defense = prev_x + 250.0;
                while defense < x {
                    intermediate_floors.push(defense);
                    defense += 250.0;
                }
            }
        }

        for intermediate in intermediate_floors {
            let intermediate_goal = match kind {
                FrontierKind::Stability => {
                    Goal::new(f64::NEG_INFINITY, Some(0.0), Some(intermediate as i32))
                }
                FrontierKind::Defense => Goal::new(f64::NEG_INFINITY, Some(intermediate), Some(0)),
            };
            measure_quick(kind, intermediate, &intermediate_goal, warm)?;
        }

        measure_quick(kind, x, floors, warm)
    };

    let mut prev_stability = first_stability;
    for stability in QUICK_STABILITY_FLOORS.iter().copied().skip(1) {
        if crate::solver::cancel::is_cancelled() {
            break;
        }
        let floors = Goal::new(f64::NEG_INFINITY, Some(0.0), Some(stability));
        if let Some(point) = measure(
            FrontierKind::Stability,
            prev_stability as f64,
            stability as f64,
            &floors,
            &mut stability_warm,
        ) {
            samples.push(point);
        }
        prev_stability = stability;
    }
    let mut prev_defense = 0.0;
    for defense in QUICK_DEFENSE_FLOORS {
        if crate::solver::cancel::is_cancelled() {
            break;
        }
        let floors = Goal::new(f64::NEG_INFINITY, Some(defense), Some(0));
        if let Some(point) = measure(
            FrontierKind::Defense,
            prev_defense,
            defense,
            &floors,
            &mut defense_warm,
        ) {
            samples.push(point);
        }
        prev_defense = defense;
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
/// not climb from identical seed sets.) See `workspace/QUICK_RANKING_DESIGN.md`.
pub fn solve_pareto_template(
    system: &System,
    balance: &Balance,
    horizon: i32,
    time_limit: u32,
    include_industry_upgrades: bool,
) -> ParetoSolve {
    solve_pareto_template_with_settings(
        system,
        balance,
        horizon,
        time_limit,
        SolverSettings::legacy(include_industry_upgrades),
    )
}

pub fn solve_pareto_template_with_settings(
    system: &System,
    balance: &Balance,
    horizon: i32,
    time_limit: u32,
    settings: SolverSettings,
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
        settings,
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
            settings,
            warm,
            &[],
            SearchProfile::TEMPLATE,
        )
    };

    for stability in QUICK_STABILITY_FLOORS.iter().copied().skip(1) {
        if crate::solver::cancel::is_cancelled() {
            break;
        }
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
        if crate::solver::cancel::is_cancelled() {
            break;
        }
        let floors = Goal::new(f64::NEG_INFINITY, Some(defense), Some(0));
        if let Some(point) = measure(FrontierKind::Defense, defense, &floors, &mut defense_warm) {
            samples.push(point);
        }
    }

    assemble_solve(samples)
}

/// Tier-0 *upper-bound* variant: first runs floor-0 greedy one-shot rationing
/// ([`per_planet_income_allocation`]; cores/SPs move in tapering chunks for
/// speed, items one at a time), then freezes that allocation and solves
/// per-planet menus for the stability and defense floor grids. A pure
/// integer DP combines those menus under the real system-average constraint:
/// `sum(metric_p - floor) >= 0` over the chosen colonized planets, with
/// uncolonized base planets allowed to skip. The returned frontiers are these
/// per-floor DP ceilings, not a searched Pareto frontier.
///
/// Scoring uses flat-left normalized AUC instead of the regular trapezoid AUC:
/// income can only fall as a floor tightens, so the left endpoint ceiling bounds
/// the true curve on each interval. Infeasible floor points contribute zero from
/// that floor onward. This gives the ranker a floor-aware "potential" ceiling
/// without calling the full joint search.
///
/// Caveats for certificate use: the one-shot rationing is exact only under
/// concavity (diminishing returns); under complementarities it can
/// under-allocate, and chunked core/SP moves coarsen the split further. Also,
/// the one-shot allocation is fixed at floor 0, so a joint optimum that
/// reallocates scarce units specifically for a tight floor could in principle
/// exceed this per-floor bound. Treat it as a near-certain ceiling gated
/// empirically (rank-agreement tau and bound/full ratio checks), not a hard
/// pruning proof.
pub fn solve_pareto_bound(
    system: &System,
    balance: &Balance,
    horizon: i32,
    time_limit: u32,
    include_industry_upgrades: bool,
) -> ParetoSolve {
    solve_pareto_bound_with_settings(
        system,
        balance,
        horizon,
        time_limit,
        SolverSettings::legacy(include_industry_upgrades),
    )
}

pub fn solve_pareto_bound_with_settings(
    system: &System,
    balance: &Balance,
    horizon: i32,
    time_limit: u32,
    settings: SolverSettings,
) -> ParetoSolve {
    let stats = std::env::var_os("SYSTEM_SOLVER_STATS").is_some();
    let t0 = std::time::Instant::now();
    let mut bound = per_planet_income_allocation(system, balance, horizon, time_limit, settings);
    let t_alloc = t0.elapsed();
    let t1 = std::time::Instant::now();
    let (stability_choices, defense_choices) = bound.floor_menus();
    let t_menus = t1.elapsed();
    if stats {
        eprintln!(
            "bound phases [{}]: alloc={:.2}s menus={:.2}s solves={}",
            system.name(),
            t_alloc.as_secs_f64(),
            t_menus.as_secs_f64(),
            bound.memo.len(),
        );
    }

    let mut stability_frontier = Vec::new();
    for floor in STABILITY_FLOORS {
        if let Some(income) = max_floor_income(&stability_choices, floor, 10 * bound.len()) {
            stability_frontier.push(ParetoPoint {
                kind: FrontierKind::Stability,
                floor: floor as f64,
                income: income as f64,
                stability: floor as f64,
                defense: 0.0,
                months: 0,
                actions: Vec::new(),
            });
        }
    }

    let mut defense_frontier = Vec::new();
    for (idx, floor) in DEFENSE_FLOORS.iter().copied().enumerate() {
        if let Some(income) = max_floor_income(&defense_choices, idx as i32, 4 * bound.len()) {
            defense_frontier.push(ParetoPoint {
                kind: FrontierKind::Defense,
                floor,
                income: income as f64,
                stability: 0.0,
                defense: floor,
                months: 0,
                actions: Vec::new(),
            });
        }
    }

    // Pooled market-share correction: the additive per-planet incomes
    // double-count shared commodity markets on duplicate-industry systems
    // (each planet was valued against a standalone denominator). The additive
    // score is a ceiling (a pooled-aware planner would re-optimize); the
    // greedy-pooled value of this fixed portfolio is a floor (the planner
    // keeps at least these plans). Interpolate between them with weight 2/3
    // on the pooled side — re-optimization recovers only part of the gap —
    // by raising the pooled/additive ratio to the 2/3 power. On the
    // 5-system benchmark any exponent in (0.55, 0.78) reproduces the full
    // sweep's ordering exactly, so 2/3 is mid-range, not knife-edge.
    let pooled_ratio = bound.pooled_deflation();
    let deflation = pooled_ratio.powf(2.0 / 3.0);
    if stats {
        eprintln!(
            "bound pooled deflation [{}]: ratio={:.3} applied={:.3}",
            system.name(),
            pooled_ratio,
            deflation,
        );
    }

    let stability_auc =
        flat_left_normalized_auc(&stability_frontier, FrontierKind::Stability) * deflation;
    let defense_auc =
        flat_left_normalized_auc(&defense_frontier, FrontierKind::Defense) * deflation;
    let score = ((stability_auc + defense_auc) / 2.0) / SCORE_INCOME_UNIT;

    ParetoSolve {
        stability_frontier,
        defense_frontier,
        stability_auc,
        defense_auc,
        score,
        recommendation: None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PlanetFloor {
    Stability(i32),
    DefenseUnits(i32),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PlanetSolveKey {
    idx: usize,
    cores: u32,
    sps: u32,
    items: Vec<u32>,
    floor: PlanetFloor,
}

#[derive(Debug, Clone)]
struct PlanetAllocation {
    sub_system: System,
    has_base_colony: bool,
    cores: u32,
    sps: u32,
    items: Vec<u32>,
}

/// A single-planet relaxed solve's income plus the metrics its plan actually
/// achieves. The achieved metrics let the menu builders skip floors a plan
/// already covers: for any floor `g <= achieved`, the same plan is feasible
/// with the same income, and the optimum can't exceed the looser floor's, so
/// `optimum(g)` equals this income exactly.
///
/// Also carries the plan end-state economy breakdown (direct income, upkeep,
/// per-commodity weighted supply) so the bound can re-score portfolios with
/// cross-planet market-share pooling: per-planet solves value exports against
/// standalone `sector + own supply` denominators, which double-counts shared
/// commodity markets when several planets export the same commodity.
#[derive(Debug, Clone, Copy)]
struct PlanetSolveOutcome {
    income: f64,
    stability: f64,
    defense: f64,
    direct_income: f64,
    upkeep: f64,
    raw_supply: [f64; Resource::COUNT],
    modded_supply: [f64; Resource::COUNT],
}

type PlanetMemoValue = (Option<PlanetSolveOutcome>, Option<SystemPlan>);

struct PerPlanetBound {
    planets: Vec<PlanetAllocation>,
    item_types: Vec<ColonyItem>,
    horizon: i32,
    time_limit: u32,
    settings: SolverSettings,
    incomes: Vec<f64>,
    memo: HashMap<PlanetSolveKey, PlanetMemoValue>,
}

impl PerPlanetBound {
    fn len(&self) -> i32 {
        self.planets.len() as i32
    }

    fn current_key(&self, idx: usize, floor: PlanetFloor) -> PlanetSolveKey {
        let planet = &self.planets[idx];
        self.key(idx, planet.cores, planet.sps, planet.items.clone(), floor)
    }

    fn solve_current(
        &mut self,
        idx: usize,
        floor: PlanetFloor,
        seed: Option<SystemPlan>,
    ) -> PlanetMemoValue {
        let key = self.current_key(idx, floor);
        self.solve_key(key, seed)
    }

    fn key(
        &self,
        idx: usize,
        cores: u32,
        sps: u32,
        items: Vec<u32>,
        floor: PlanetFloor,
    ) -> PlanetSolveKey {
        PlanetSolveKey {
            idx,
            cores,
            sps,
            items,
            floor,
        }
    }

    /// Ratio of pooled (cross-planet market share) to additive income for the
    /// final floor-0 portfolio, in `(0, 1]`. Per-planet solves value exports
    /// against standalone `sector + own supply` denominators; summing them
    /// double-counts shared commodity markets on duplicate-industry systems.
    ///
    /// Pooling the additive-optimal portfolio as-is is unfairly pessimistic
    /// (a pooled-aware planner would not keep every duplicate producer paying
    /// upkeep for a diluted share), so the pooled side greedily keeps only
    /// planets with a positive marginal pooled contribution — the subset a
    /// pooled-aware planner would retain from these plans. Pooling is
    /// subadditive, so the ratio is `<= 1`; it deflates the ranking score
    /// (the per-floor frontier incomes stay additive ceilings).
    fn pooled_deflation(&self) -> f64 {
        let mut outcomes: Vec<PlanetSolveOutcome> = Vec::with_capacity(self.planets.len());
        for idx in 0..self.planets.len() {
            let key = self.current_key(idx, PlanetFloor::Stability(0));
            if let Some((Some(outcome), _)) = self.memo.get(&key) {
                outcomes.push(*outcome);
            }
        }

        let additive: f64 = outcomes.iter().map(|o| o.income).sum();
        if outcomes.is_empty() || additive <= 0.0 {
            return 1.0;
        }

        let pooled_value = |chosen: &[bool]| -> f64 {
            let mut net = 0.0;
            let mut raw = [0.0f64; Resource::COUNT];
            let mut modded = [0.0f64; Resource::COUNT];
            for (outcome, keep) in outcomes.iter().zip(chosen) {
                if !keep {
                    continue;
                }
                net += outcome.direct_income - outcome.upkeep;
                for i in 0..Resource::COUNT {
                    raw[i] += outcome.raw_supply[i];
                    modded[i] += outcome.modded_supply[i];
                }
            }
            for resource in Resource::ALL {
                let i = resource as usize;
                if raw[i] > 0.0 {
                    net += resource.market_value() as f64 * modded[i]
                        / (resource.sector_supply() as f64 + raw[i]);
                }
            }
            net
        };

        let mut chosen = vec![false; outcomes.len()];
        let mut current = 0.0;
        loop {
            let mut best: Option<(usize, f64)> = None;
            for idx in 0..outcomes.len() {
                if chosen[idx] {
                    continue;
                }
                chosen[idx] = true;
                let candidate = pooled_value(&chosen);
                chosen[idx] = false;
                if candidate > current && best.is_none_or(|(_, value)| candidate > value) {
                    best = Some((idx, candidate));
                }
            }
            let Some((idx, value)) = best else { break };
            chosen[idx] = true;
            current = value;
        }

        (current / additive).clamp(0.0, 1.0)
    }

    fn solve_key(&mut self, key: PlanetSolveKey, seed: Option<SystemPlan>) -> PlanetMemoValue {
        if let Some(value) = self.memo.get(&key) {
            return value.clone();
        }

        let value = solve_key_for_planet(
            &self.planets[key.idx],
            &self.item_types,
            self.horizon,
            self.time_limit,
            self.settings,
            &key,
            seed,
        );
        self.memo.insert(key, value.clone());
        value
    }

    /// Builds the stability menu (floors 0..=10 — stability is
    /// integer-valued, so that covers every local value the DP can see) and
    /// the defense bucket menu (each grid floor's income credited with the
    /// next grid value, saturating top bucket — over-crediting keeps the DP a
    /// relaxation) for each planet after floor-0 one-shot rationing.
    ///
    /// Floors already covered by a previous solve's *achieved* metric reuse
    /// its income without a fresh search (the plan is feasible there with the
    /// same income, and a looser floor's optimum can't be lower — so the
    /// optimum is exactly that income); the emitted menus are identical to
    /// the solve-every-floor ones. Infeasibility is monotone, so the first
    /// infeasible floor ends a menu.
    ///
    /// Both kinds are solved in a single
    /// parallel pass (2n independent tasks: each planet's stability chain and
    /// defense chain). Within a task the floor chain stays sequential and
    /// warm-chained, so results are identical to building the menus one
    /// kind at a time.
    fn floor_menus(&mut self) -> (Vec<Vec<FloorChoice>>, Vec<Vec<FloorChoice>>) {
        #[derive(Clone, Copy, PartialEq)]
        enum MenuKind {
            Stability,
            Defense,
        }

        let memo = self.memo.clone();
        let item_types = self.item_types.clone();
        let horizon = self.horizon;
        let time_limit = self.time_limit;
        let settings = self.settings;
        let last_bucket = DEFENSE_FLOORS.len() - 1;

        let mut tasks = Vec::with_capacity(self.planets.len() * 2);
        for (idx, planet) in self.planets.iter().enumerate() {
            tasks.push((idx, MenuKind::Stability, planet.clone()));
            tasks.push((idx, MenuKind::Defense, planet.clone()));
        }

        let mut all: Vec<_> = tasks
            .into_par_iter()
            .map(|(idx, kind, planet)| {
                let mut local_memo = HashMap::new();
                let mut choices = Vec::new();
                if !planet.has_base_colony {
                    choices.push(FloorChoice::skip());
                }
                let floor0_key = key_for_planet(idx, &planet, PlanetFloor::Stability(0));
                let mut warm = memo.get(&floor0_key).and_then(|(_, plan)| plan.clone());
                let mut solve = |floor: PlanetFloor,
                                 warm: &mut Option<SystemPlan>,
                                 local_memo: &mut HashMap<PlanetSolveKey, PlanetMemoValue>|
                 -> Option<PlanetSolveOutcome> {
                    let key = key_for_planet(idx, &planet, floor);
                    let value = solve_key_with_memos(
                        &planet,
                        &item_types,
                        horizon,
                        time_limit,
                        settings,
                        &memo,
                        local_memo,
                        key,
                        warm.clone(),
                    );
                    *warm = value.1.clone();
                    value.0
                };
                match kind {
                    MenuKind::Stability => {
                        let mut stability = 0;
                        while stability <= 10 {
                            let Some(outcome) = solve(
                                PlanetFloor::Stability(stability),
                                &mut warm,
                                &mut local_memo,
                            ) else {
                                break;
                            };
                            let covered = (outcome.stability.floor() as i32).clamp(stability, 10);
                            for floor in stability..=covered {
                                choices.push(FloorChoice::colonized(outcome.income, floor));
                            }
                            stability = covered + 1;
                        }
                    }
                    MenuKind::Defense => {
                        let mut bucket = 0;
                        while bucket <= last_bucket {
                            let Some(outcome) = solve(
                                PlanetFloor::DefenseUnits(bucket as i32),
                                &mut warm,
                                &mut local_memo,
                            ) else {
                                break;
                            };
                            let covered = ((outcome.defense / 250.0).floor() as usize)
                                .clamp(bucket, last_bucket);
                            for b in bucket..=covered {
                                let metric = if b == last_bucket {
                                    i32::MAX / 4
                                } else {
                                    b as i32 + 1
                                };
                                choices.push(FloorChoice::colonized(outcome.income, metric));
                            }
                            bucket = covered + 1;
                        }
                    }
                }
                let local_entries: Vec<_> = local_memo.into_iter().collect();
                (idx, kind, choices, local_entries)
            })
            .collect();

        all.sort_by_key(|(idx, kind, _, _)| (*idx, *kind == MenuKind::Defense));
        let mut stability_choices = Vec::with_capacity(self.planets.len());
        let mut defense_choices = Vec::with_capacity(self.planets.len());
        for (_, kind, planet_choices, entries) in all {
            match kind {
                MenuKind::Stability => stability_choices.push(planet_choices),
                MenuKind::Defense => defense_choices.push(planet_choices),
            }
            self.memo.extend(entries);
        }
        (stability_choices, defense_choices)
    }
}

fn per_planet_income_allocation(
    system: &System,
    balance: &Balance,
    horizon: i32,
    time_limit: u32,
    settings: SolverSettings,
) -> PerPlanetBound {
    let mut hashes: Vec<u64> = system.planets().keys().copied().collect();
    hashes.sort_unstable();
    let subs: Vec<System> = hashes
        .iter()
        .map(|h| single_planet_system(system, *h))
        .collect();
    let n = subs.len();

    let mut item_types: Vec<ColonyItem> = balance.colony_items().keys().copied().collect();
    item_types.sort_unstable();
    let item_totals: Vec<u32> = item_types
        .iter()
        .map(|it| *balance.colony_items().get(it).unwrap_or(&0))
        .collect();
    let n_items = item_types.len();

    let mut bound = PerPlanetBound {
        planets: subs
            .into_iter()
            .map(|sub_system| PlanetAllocation {
                has_base_colony: sub_system.planets().values().any(|p| p.has_colony()),
                sub_system,
                cores: 0,
                sps: 0,
                items: vec![0u32; n_items],
            })
            .collect(),
        item_types,
        horizon,
        time_limit,
        settings,
        incomes: Vec::new(),
        memo: HashMap::new(),
    };

    let initial_tasks: Vec<_> = bound
        .planets
        .iter()
        .cloned()
        .enumerate()
        .map(|(p, planet)| {
            let key = key_for_planet(p, &planet, PlanetFloor::Stability(0));
            (p, planet, key)
        })
        .collect();
    let item_types = bound.item_types.clone();
    let initial: Vec<_> = initial_tasks
        .into_par_iter()
        .map(|(p, planet, key)| {
            let value = solve_key_for_planet(
                &planet,
                &item_types,
                horizon,
                time_limit,
                settings,
                &key,
                None,
            );
            (p, key, value)
        })
        .collect();

    bound.incomes = vec![0.0; n];
    for (p, key, value) in initial {
        bound.incomes[p] = value.0.map_or(0.0, |o| o.income);
        bound.memo.insert(key, value);
    }

    let mut cores_rem = balance.alpha_cores();
    let mut sps_rem = balance.story_points();
    let mut items_rem = item_totals;

    #[derive(Clone, Copy)]
    enum Unit {
        Core(u32),
        Sp(u32),
        Item(usize),
    }

    struct Candidate {
        p: usize,
        unit: Unit,
        key: PlanetSolveKey,
    }

    // Cores and SPs are allocated in chunks (4 while the pool is deep, then 2,
    // then 1) instead of one at a time: the greedy's acceptance chain is
    // inherently sequential, and at DadGummit-scale balances (~70 units) the
    // unit-by-unit loop dominated bound wall time. Chunking changes which
    // allocation the greedy lands on (it was already heuristic — see the
    // certificate caveats on `solve_pareto_bound`), so it is gated by the
    // rank-agreement validation, not by an exactness argument.
    let chunk_for = |rem: u32| -> u32 {
        if rem >= 16 {
            4
        } else if rem >= 4 {
            2
        } else {
            1
        }
    };

    loop {
        let core_chunk = chunk_for(cores_rem);
        let sp_chunk = chunk_for(sps_rem);
        let mut candidates = Vec::new();
        for p in 0..n {
            if cores_rem > 0 {
                candidates.push(Candidate {
                    p,
                    unit: Unit::Core(core_chunk),
                    key: bound.key(
                        p,
                        bound.planets[p].cores + core_chunk,
                        bound.planets[p].sps,
                        bound.planets[p].items.clone(),
                        PlanetFloor::Stability(0),
                    ),
                });
            }
            if sps_rem > 0 {
                candidates.push(Candidate {
                    p,
                    unit: Unit::Sp(sp_chunk),
                    key: bound.key(
                        p,
                        bound.planets[p].cores,
                        bound.planets[p].sps + sp_chunk,
                        bound.planets[p].items.clone(),
                        PlanetFloor::Stability(0),
                    ),
                });
            }
            for j in 0..n_items {
                if items_rem[j] > 0 {
                    let mut trial = bound.planets[p].items.clone();
                    trial[j] += 1;
                    candidates.push(Candidate {
                        p,
                        unit: Unit::Item(j),
                        key: bound.key(
                            p,
                            bound.planets[p].cores,
                            bound.planets[p].sps,
                            trial,
                            PlanetFloor::Stability(0),
                        ),
                    });
                }
            }
        }
        if candidates.is_empty() {
            break;
        }

        let misses: Vec<_> = candidates
            .iter()
            .enumerate()
            .filter(|(_, cand)| !bound.memo.contains_key(&cand.key))
            .map(|(scan, cand)| {
                let seed = bound
                    .memo
                    .get(&bound.current_key(cand.p, PlanetFloor::Stability(0)))
                    .and_then(|(_, plan)| plan.clone());
                (scan, bound.planets[cand.p].clone(), cand.key.clone(), seed)
            })
            .collect();
        let item_types = bound.item_types.clone();
        let solved: Vec<_> = misses
            .into_par_iter()
            .map(|(scan, planet, key, seed)| {
                let value = solve_key_for_planet(
                    &planet,
                    &item_types,
                    horizon,
                    time_limit,
                    settings,
                    &key,
                    seed,
                );
                (scan, key, value)
            })
            .collect();
        for (_, key, value) in solved {
            bound.memo.insert(key, value);
        }

        // Multi-accept: every planet's gains are independent of other
        // planets' allocations, so granting each planet its best unit in the
        // same round reproduces several consecutive greedy steps exactly —
        // inter-planet competition only matters through pool depletion, which
        // the gain-ordered acceptance below resolves (a winner whose pool ran
        // out this round simply retries next round at the smaller chunk).
        let mut winners: Vec<(f64, usize, Unit)> = Vec::new();
        for cand in &candidates {
            let cur = bound.incomes[cand.p];
            let gain = bound
                .memo
                .get(&cand.key)
                .and_then(|(outcome, _)| *outcome)
                .map_or(0.0, |o| o.income)
                - cur;
            if gain > 1.0 {
                match winners.iter_mut().find(|(_, p, _)| *p == cand.p) {
                    Some(entry) if gain > entry.0 => *entry = (gain, cand.p, cand.unit),
                    Some(_) => {}
                    None => winners.push((gain, cand.p, cand.unit)),
                }
            }
        }
        if winners.is_empty() {
            break;
        }
        // Highest gain first; planet index breaks exact ties deterministically
        // (matching the old scan order, which preferred the lower index).
        winners.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

        let mut accepted_any = false;
        for (_, p, unit) in winners {
            let fits = match unit {
                Unit::Core(chunk) => cores_rem >= chunk,
                Unit::Sp(chunk) => sps_rem >= chunk,
                Unit::Item(j) => items_rem[j] > 0,
            };
            if !fits {
                continue;
            }
            let accepted_seed = bound
                .memo
                .get(&bound.current_key(p, PlanetFloor::Stability(0)))
                .and_then(|(_, plan)| plan.clone());
            match unit {
                Unit::Core(chunk) => {
                    bound.planets[p].cores += chunk;
                    cores_rem -= chunk;
                }
                Unit::Sp(chunk) => {
                    bound.planets[p].sps += chunk;
                    sps_rem -= chunk;
                }
                Unit::Item(j) => {
                    bound.planets[p].items[j] += 1;
                    items_rem[j] -= 1;
                }
            }
            accepted_any = true;
            bound.incomes[p] = bound
                .solve_current(p, PlanetFloor::Stability(0), accepted_seed)
                .0
                .map_or(0.0, |o| o.income);
        }
        if !accepted_any {
            break;
        }
    }

    bound
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

fn key_for_planet(idx: usize, planet: &PlanetAllocation, floor: PlanetFloor) -> PlanetSolveKey {
    PlanetSolveKey {
        idx,
        cores: planet.cores,
        sps: planet.sps,
        items: planet.items.clone(),
        floor,
    }
}

#[allow(clippy::too_many_arguments)]
fn solve_key_for_planet(
    planet: &PlanetAllocation,
    item_types: &[ColonyItem],
    horizon: i32,
    time_limit: u32,
    settings: SolverSettings,
    key: &PlanetSolveKey,
    seed: Option<SystemPlan>,
) -> PlanetMemoValue {
    let alloc: Vec<(ColonyItem, u32)> = item_types
        .iter()
        .copied()
        .zip(key.items.iter().copied())
        .filter(|(_, count)| *count > 0)
        .collect();
    solve_planet_income_optional(
        &planet.sub_system,
        key.cores,
        key.sps,
        &alloc,
        key.floor,
        horizon,
        time_limit,
        settings,
        seed,
    )
}

#[allow(clippy::too_many_arguments)]
fn solve_key_with_memos(
    planet: &PlanetAllocation,
    item_types: &[ColonyItem],
    horizon: i32,
    time_limit: u32,
    settings: SolverSettings,
    shared_memo: &HashMap<PlanetSolveKey, PlanetMemoValue>,
    local_memo: &mut HashMap<PlanetSolveKey, PlanetMemoValue>,
    key: PlanetSolveKey,
    seed: Option<SystemPlan>,
) -> PlanetMemoValue {
    if let Some(value) = shared_memo.get(&key) {
        return value.clone();
    }
    if let Some(value) = local_memo.get(&key) {
        return value.clone();
    }
    let value = solve_key_for_planet(
        planet, item_types, horizon, time_limit, settings, &key, seed,
    );
    local_memo.insert(key, value.clone());
    value
}

#[allow(clippy::too_many_arguments)]
fn solve_planet_income_optional(
    sub_system: &System,
    cores: u32,
    sps: u32,
    items: &[(ColonyItem, u32)],
    floor: PlanetFloor,
    horizon: i32,
    time_limit: u32,
    settings: SolverSettings,
    seed: Option<SystemPlan>,
) -> PlanetMemoValue {
    let mut balance = Balance::new(1e15, sps, cores);
    for (item, count) in items {
        for _ in 0..*count {
            balance.add_colony_item(*item);
        }
    }
    let (kind, point_floor, floors) = match floor {
        PlanetFloor::Stability(stability) => (
            FrontierKind::Stability,
            stability as f64,
            Goal::new(f64::NEG_INFINITY, Some(0.0), Some(stability)),
        ),
        PlanetFloor::DefenseUnits(units) => (
            FrontierKind::Defense,
            units as f64 * 250.0,
            Goal::new(f64::NEG_INFINITY, Some(units as f64 * 250.0), Some(0)),
        ),
    };
    let mut warm = seed;
    let point = measure_point_seeded(
        sub_system,
        &balance,
        kind,
        point_floor,
        &floors,
        horizon,
        time_limit,
        settings,
        &mut warm,
        &[],
        SearchProfile::BOUND,
    );
    let outcome = point.map(|point| {
        // Replay the chosen plan to extract the end-state economy breakdown
        // for pooled (cross-planet market share) re-scoring at system scope.
        let mut replay = State::new(balance.clone(), sub_system.clone());
        for action in &point.actions {
            replay.apply_action_raw(action, false);
        }
        let mut direct_income = 0.0;
        let mut upkeep = 0.0;
        let mut raw_supply = [0.0f64; Resource::COUNT];
        let mut modded_supply = [0.0f64; Resource::COUNT];
        for planet in replay.system().planets().values() {
            let economy = planet.economy();
            direct_income += economy.direct_income;
            upkeep += economy.upkeep;
            for &(resource, raw, modded) in &economy.exports {
                raw_supply[resource as usize] += raw;
                modded_supply[resource as usize] += modded;
            }
        }
        PlanetSolveOutcome {
            income: point.income,
            stability: point.stability,
            defense: point.defense,
            direct_income,
            upkeep,
            raw_supply,
            modded_supply,
        }
    });
    (outcome, warm)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FloorChoice {
    income: i64,
    metric: i32,
    colonized: bool,
}

impl FloorChoice {
    fn colonized(income: f64, metric: i32) -> Self {
        Self {
            income: income.max(0.0).ceil() as i64,
            metric,
            colonized: true,
        }
    }

    fn skip() -> Self {
        Self {
            income: 0,
            metric: 0,
            colonized: false,
        }
    }
}

/// Exact integer DP for one average-metric floor. Each planet contributes one
/// menu choice, or a skip choice when the caller supplied one. Colonized choices
/// add slack `metric - floor`; reaching non-negative final slack satisfies the
/// system-average floor. Positive slack is clamped because surplus beyond what
/// the remaining planets could consume is equivalent for feasibility, while the
/// objective remains the maximum total income.
fn max_floor_income(
    choices_by_planet: &[Vec<FloorChoice>],
    floor: i32,
    slack_cap: i32,
) -> Option<i64> {
    let mut states: HashMap<(i32, bool), i64> = HashMap::from([((0, false), 0)]);
    for choices in choices_by_planet {
        let mut next: HashMap<(i32, bool), i64> = HashMap::new();
        for (&(slack, has_colony), &income) in &states {
            for choice in choices {
                let (new_slack, new_has_colony) = if choice.colonized {
                    (
                        slack
                            .saturating_add(choice.metric.saturating_sub(floor))
                            .min(slack_cap),
                        true,
                    )
                } else {
                    (slack, has_colony)
                };
                let new_income = income.saturating_add(choice.income);
                next.entry((new_slack, new_has_colony))
                    .and_modify(|best| *best = (*best).max(new_income))
                    .or_insert(new_income);
            }
        }
        states = next;
    }

    states
        .into_iter()
        .filter(|((slack, has_colony), _)| *slack >= 0 && (floor <= 0 || *has_colony))
        .map(|(_, income)| income)
        .max()
}

fn flat_left_normalized_auc(frontier: &[ParetoPoint], kind: FrontierKind) -> f64 {
    let (min_x, max_x) = frontier_bounds(kind);
    let span = max_x - min_x;
    if span <= 0.0 {
        return 0.0;
    }

    let floors: Vec<f64> = match kind {
        FrontierKind::Stability => STABILITY_FLOORS.iter().map(|floor| *floor as f64).collect(),
        FrontierKind::Defense => DEFENSE_FLOORS.to_vec(),
    };
    let mut feasible_tail = true;
    let raw: f64 = floors
        .windows(2)
        .map(|pair| {
            let y = if feasible_tail {
                match frontier.iter().find(|point| point.frontier_x() == pair[0]) {
                    Some(point) => point.income.max(0.0),
                    None => {
                        feasible_tail = false;
                        0.0
                    }
                }
            } else {
                0.0
            };
            (pair[1] - pair[0]).max(0.0) * y
        })
        .sum();
    raw / span
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
    settings: SolverSettings,
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
        settings,
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
    settings: SolverSettings,
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
        settings,
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

    if std::env::var_os("SYSTEM_SOLVER_RANK_POINTS").is_some() {
        eprintln!(
            "point {:?} floor={} income={:.0} months={} profile(top={},nodes={},warm_only={})",
            kind,
            floor,
            replay.balance().net_income(),
            result.cost,
            profile.top_seed_climbs,
            profile.max_nodes_per_seed,
            profile.warm_seeds_only,
        );
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
    // The first frontier point's plan is feasible at every looser floor with
    // the same income, so the curve extends flat from min_x to that point.
    // Without this strip, two equal-income plans score differently purely by
    // the achieved metric of the unconstrained optimum (an artifact that made
    // quick/full ratios incomparable across systems). No flat-right analogue:
    // beyond the last achieved x feasibility is unknown, so it stays zero.
    let left = frontier.first().map_or(0.0, |first| {
        let x0 = first.frontier_x().clamp(min_x, max_x);
        (x0 - min_x).max(0.0) * first.income.max(0.0)
    });
    left + frontier
        .windows(2)
        .map(|pair| {
            let x0 = pair[0].frontier_x().clamp(min_x, max_x);
            let x1 = pair[1].frontier_x().clamp(min_x, max_x);
            let y0 = pair[0].income.max(0.0);
            let y1 = pair[1].income.max(0.0);
            (x1 - x0).max(0.0) * (y0 + y1) / 2.0
        })
        .sum::<f64>()
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
    use super::{
        flat_left_normalized_auc, max_floor_income, normalized_auc, pareto_frontier, raw_auc,
        FloorChoice, FrontierKind, ParetoPoint,
    };

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

    #[test]
    fn floor_dp_allows_compensation_across_planets() {
        let choices = vec![
            vec![FloorChoice::colonized(100.0, 10)],
            vec![FloorChoice::colonized(80.0, 0)],
        ];
        assert_eq!(max_floor_income(&choices, 5, 20), Some(180));
    }

    #[test]
    fn floor_dp_skip_option_excludes_uncolonized_planets_from_average() {
        let choices = vec![
            vec![FloorChoice::skip(), FloorChoice::colonized(100.0, 0)],
            vec![FloorChoice::colonized(10.0, 5)],
        ];
        assert_eq!(max_floor_income(&choices, 5, 20), Some(10));
    }

    #[test]
    fn floor_dp_existing_colonies_are_mandatory_when_no_skip_is_present() {
        let choices = vec![
            vec![FloorChoice::colonized(100.0, 0)],
            vec![FloorChoice::colonized(10.0, 10)],
        ];
        assert_eq!(max_floor_income(&choices, 5, 20), Some(110));
    }

    #[test]
    fn floor_dp_reports_infeasible_floor() {
        let choices = vec![
            vec![FloorChoice::colonized(100.0, 4)],
            vec![FloorChoice::colonized(10.0, 4)],
        ];
        assert_eq!(max_floor_income(&choices, 5, 20), None);
    }

    #[test]
    fn floor_dp_handles_defense_overcredit_and_top_bucket() {
        let choices = vec![
            vec![FloorChoice::colonized(100.0, 1)],
            vec![FloorChoice::colonized(50.0, i32::MAX / 4)],
        ];
        assert_eq!(max_floor_income(&choices, 1, 8), Some(150));
        assert_eq!(max_floor_income(&choices, 4, 8), Some(150));
    }

    #[test]
    fn flat_left_auc_uses_left_endpoint_and_zeros_infeasible_tail() {
        let frontier = vec![point(5.0, 100.0), point(6.0, 80.0), point(8.0, 40.0)];
        assert_eq!(
            flat_left_normalized_auc(&frontier, FrontierKind::Stability),
            36.0
        );
    }
}
