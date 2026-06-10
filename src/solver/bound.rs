//! Admissible upper bound on the maximize objective via a **budget relaxation**.
//!
//! The greedy two-level solver ([`crate::solver::decomp`]) converges fast but is
//! a heuristic: its value is *not monotone in plan-inclusion* because builds cost
//! credits/story-points/cores and take time, so dropping a target can free the
//! budget to reach a better one sooner. That schedule-dependent ruggedness is the
//! root of the maximize local-minima and the hand-found edge cases (see
//! `MAXIMIZE_LOCAL_MINIMA.md`). Patching those one at a time never ends; the way
//! off that treadmill is a *certificate* — how far the greedy result can possibly
//! be from optimal.
//!
//! This module computes one side of that certificate cheaply. Every gating
//! constraint the action generator enforces is one of two kinds:
//!
//! * **budget** — credits (`balance.credits() >= build_cost`), story points
//!   (improvements), alpha cores (cores / admin upgrade), and colony items
//!   (installs); all read off [`Balance`];
//! * **physical** — colony size / industry slots / build time, which only advance
//!   with real game-months of growth.
//!
//! But the two budget resources are *not* alike, and this distinction is the
//! whole game:
//!
//! * **Credits are renewable** — colonies generate income, so given enough
//!   *time* you can afford anything. The credit constraint is therefore really a
//!   constraint on *timing and ordering* (build the income engine first so it
//!   funds the rest), which is exactly the schedule-dependent ruggedness that
//!   traps the greedy.
//! * **Story points, alpha cores and colony items are one-shot and scarce** — you
//!   start with a literal handful (e.g. 1 alpha core) and they do not regrow. An
//!   *optimal* solver must respect those caps too. Relaxing them would bound a
//!   different, far easier problem (an alpha core on every facility trivially
//!   multiplies income), producing a vacuously loose number that says nothing
//!   about how well the greedy schedules the *real* problem.
//!
//! So the bound relaxes **credits only**, holding story points, alpha cores and
//! colony items at the real starting amounts. Removing just the renewable
//! constraint yields an optimum that can only be **higher** than the real one
//! (removing a constraint never lowers a max), so for the same system, goal and
//! horizon:
//!
//! ```text
//! greedy_value  <=  true_optimum  <=  bound_value
//! ```
//!
//! and `bound_value - greedy_value` is an upper estimate of the headroom the
//! greedy leaves on the *credit-timing* axis — precisely the axis the ruggedness
//! comes from. Crucially, the relaxation *removes* that ruggedness: with money no
//! object, value becomes monotone in helpful structure (subject to the still-real
//! core/SP/item caps), so the same VND climb reliably reaches the relaxed optimum
//! and the bound is trustworthy.
//!
//! Caveats, stated plainly so the gap is read honestly:
//! * It does **not** bound headroom from the *representation* (e.g. a timed
//!   "hazard pay on early, off later" policy the static `SystemPlan` cannot
//!   express). Those are a separate class; this isolates the credit-timing axis.
//! * Build *time* and growth are kept real, so over a long horizon the bound is
//!   tight (everything affordable still has to be built and grown). That is
//!   deliberate: it keeps the bound a believable ceiling rather than a vacuous
//!   "all facilities, instantly, at max size" number.

use crate::solver::state::Balance;

/// The credit-only budget relaxation: the caller's real starting balance with
/// credits inflated so high nothing is ever credit-gated, but story points,
/// alpha cores and colony items left exactly as they are. See the module docs
/// for why only the *renewable* resource is relaxed.
pub fn credits_relaxed(base: &Balance) -> Balance {
    // Credits are f64 and only ever spent in build-cost chunks; 1e15 outlasts any
    // plausible plan without risking precision in the income math (which is
    // credit-balance-independent anyway).
    let mut balance = base.clone();
    balance.set_credits(1e15);
    balance
}

/// One greedy-vs-bound measurement at a single (system, floor) point.
#[derive(Debug, Clone)]
pub struct BoundRow {
    pub system: String,
    pub kind: &'static str,
    pub floor: f64,
    /// Achieved income under the real starting balance (`None` if no feasible
    /// plan held the floors).
    pub greedy_income: Option<f64>,
    /// Achieved income under the unlimited balance.
    pub bound_income: Option<f64>,
    pub greedy_months: Option<i32>,
    pub bound_months: Option<i32>,
}

impl BoundRow {
    /// Absolute headroom `bound - greedy`, when both points are feasible.
    pub fn gap(&self) -> Option<f64> {
        Some(self.bound_income? - self.greedy_income?)
    }

    /// Headroom as a percentage of the greedy income. `None` if either point is
    /// infeasible or the greedy income is non-positive (percentage undefined).
    pub fn gap_pct(&self) -> Option<f64> {
        let g = self.greedy_income?;
        let b = self.bound_income?;
        (g > 0.0).then(|| (b - g) / g * 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credits_relaxed_inflates_only_credits() {
        let mut real = Balance::new(5_000_000.0, 5, 1);
        real.add_colony_item(crate::constants::ColonyItem::CorruptedNanoforge);
        let relaxed = credits_relaxed(&real);
        // Credits inflated...
        assert!(relaxed.credits() > real.credits());
        // ...but the scarce one-shot resources are untouched.
        assert_eq!(relaxed.story_points(), real.story_points());
        assert_eq!(relaxed.alpha_cores(), real.alpha_cores());
        assert_eq!(
            relaxed
                .colony_items()
                .get(&crate::constants::ColonyItem::CorruptedNanoforge),
            real.colony_items()
                .get(&crate::constants::ColonyItem::CorruptedNanoforge),
        );
    }

    #[test]
    fn gap_is_none_when_either_side_infeasible() {
        let row = BoundRow {
            system: "x".into(),
            kind: "stability",
            floor: 6.0,
            greedy_income: None,
            bound_income: Some(100.0),
            greedy_months: None,
            bound_months: Some(10),
        };
        assert!(row.gap().is_none());
        assert!(row.gap_pct().is_none());
    }

    #[test]
    fn gap_pct_undefined_for_nonpositive_greedy() {
        let row = BoundRow {
            system: "x".into(),
            kind: "stability",
            floor: 6.0,
            greedy_income: Some(-10.0),
            bound_income: Some(100.0),
            greedy_months: Some(5),
            bound_months: Some(5),
        };
        assert_eq!(row.gap(), Some(110.0));
        assert!(row.gap_pct().is_none());
    }
}
