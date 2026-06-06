//! The solver goal and the shared search-result type.
//!
//! `Goal` is a threshold in output space (net income, and optional average
//! ground defense / stability). It is shared by the live joint solver
//! ([`crate::solver::decomp`]) and the archived IDA* solver — the IDA*-specific
//! admissible-bound methods live alongside that solver in
//! [`crate::solver::archive::astar`] as a separate `impl Goal` block.

use crate::solver::state::{Action, State};

/// One of the three system metrics the solver can optimize. Used by the
/// maximize-mode search ([`crate::solver::decomp`]) to name *which* output to
/// push as high as possible, while the others are held above their floors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Income,
    Defense,
    Stability,
}

impl Metric {
    /// The metric's current value in `state`, in its native units (credits/month
    /// for income, raw average for defense/stability).
    pub fn value(self, state: &State) -> f64 {
        match self {
            Metric::Income => state.balance().net_income(),
            Metric::Defense => state.system().avg_ground_defense(),
            Metric::Stability => state.system().avg_stability(),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Metric::Income => "income",
            Metric::Defense => "defense",
            Metric::Stability => "stability",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Goal {
    pub(crate) min_net_income: f64,
    pub(crate) min_ground_defense: Option<f64>,
    pub(crate) min_stability: Option<i32>,
}

impl Goal {
    pub fn new(min_net_income: f64, min_ground_defense: Option<f64>, min_stability: Option<i32>) -> Self {
        Self {
            min_net_income,
            min_ground_defense,
            min_stability,
        }
    }

    pub fn is_satisfied(&self, state: &State) -> bool {
        if state.balance().net_income() < self.min_net_income {
            return false;
        }

        let system = state.system();

        if let Some(min_defense) = self.min_ground_defense {
            if system.avg_ground_defense() < min_defense {
                return false;
            }
        }

        if let Some(min_stability) = self.min_stability {
            if system.avg_stability() < min_stability as f64 {
                return false;
            }
        }

        println!("\nSatisfied with balance: {:?}", state.balance());
        println!("Satisfied with ground defense: {}", system.avg_ground_defense());
        println!("Satisfied with stability: {}", system.avg_stability());

        true
    }

    /// Like [`Goal::is_satisfied`] but without the diagnostic prints. The
    /// decomposition solver evaluates this thousands of times while searching
    /// over build plans, so it must stay quiet.
    pub fn is_satisfied_quiet(&self, state: &State) -> bool {
        if state.balance().net_income() < self.min_net_income {
            return false;
        }

        let system = state.system();

        if let Some(min_defense) = self.min_ground_defense {
            if system.avg_ground_defense() < min_defense {
                return false;
            }
        }

        if let Some(min_stability) = self.min_stability {
            if system.avg_stability() < min_stability as f64 {
                return false;
            }
        }

        true
    }
}

/// The result of a solver run: the action sequence found (if any), its cost in
/// months, and search bookkeeping. Named for the original IDA* solver; now the
/// common result type for every solver in this crate.
#[derive(Debug, Clone)]
pub struct AStarSearchResult {
    pub solution: Option<Vec<Action>>,
    pub cost: i32,
    pub cutoff_occurred: bool,
    pub nodes_searched: u32,
    pub nodes_pruned_by_bound: u32,
}
