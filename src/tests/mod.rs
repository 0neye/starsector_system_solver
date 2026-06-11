//! In-crate test suite.
//!
//! These are compiled only under `cfg(test)` and live inside the crate (rather
//! than in `tests/`) because they exercise `pub(crate)` internals such as
//! facility build-day counters and the solver's apply/undo invariants, which an
//! external integration test could not reach.
//!
//! ## Layout
//! Test files mirror the `src/` modules they exercise:
//! - [`support`]  — shared fixtures and builders (start here when adding tests)
//! - [`planet`]   — colony facility gating and resource production
//! - [`facility`] — facility construction/upgrade bookkeeping
//! - [`solver`]   — action-sequence hashing and apply/undo round-trips
//! - [`system`]   — system-wide aggregates (averages)
//! - [`parser`]   — CSV loading (e.g. infrastructure-type spellings)
//!
//! ## Adding a test
//! 1. Reuse a builder from [`support`] (`PlanetBuilder`, `colonized_state`, …);
//!    add a new helper there rather than duplicating setup.
//! 2. Put the test in the file matching the module under test.
//! 3. Name it after the behavior it guards; if it locks in a specific bug fix,
//!    note the origin in a doc comment so the intent survives refactors.
//!
//! Run with `cargo test`.

mod support;

mod facility;
mod parser;
mod planet;
mod rank;
mod solver;
mod system;
