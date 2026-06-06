//! Archived solvers, superseded by the joint decomposition solver in
//! [`crate::solver::decomp`]. These are kept for benchmarking and historical
//! comparison and are not on the default path.
//!
//! - [`astar`] — the goal-directed IDA* solver.
//! - [`split`] — the per-planet decomposition entry point.
//! - [`dfs`]   — the original score-maximizing iterative-deepening DFS and the
//!   greedy linear simulator.

pub mod astar;
pub mod dfs;
pub mod split;
