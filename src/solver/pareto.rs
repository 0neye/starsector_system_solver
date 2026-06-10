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
/// + 2 defense points instead of 11), a reduced-effort anchor solve, and
/// node-capped repair climbs warm-started along each chain. Deterministic, and
/// shares the frontier/AUC/score code with the full sweep so the two can't
/// drift. Every search budget is a strict reduction of the full sweep's, so
/// the returned score is a lower bound on the full-sweep score. See
/// `QUICK_RANKING_DESIGN.md`.
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
