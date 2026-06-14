//! Process-wide cooperative cancellation for the solver.
//!
//! Both the CLI and the TUI run at most one solve at a time, so a single
//! global flag is sufficient. The hot search loops poll [`is_cancelled`]
//! alongside their node-budget and wall-clock deadline checks and return
//! their best-so-far result when it is set. Callers that want to interrupt a
//! solve call [`request`] from another thread; whoever starts a fresh solve
//! is responsible for calling [`clear`] first (the TUI job runner does this).

use std::sync::atomic::{AtomicBool, Ordering};

static CANCEL: AtomicBool = AtomicBool::new(false);

/// Ask the in-flight solve (if any) to stop at the next poll point.
pub fn request() {
    CANCEL.store(true, Ordering::Relaxed);
}

/// Reset the flag. Call before starting a new solve.
pub fn clear() {
    CANCEL.store(false, Ordering::Relaxed);
}

/// Polled by the search loops.
pub fn is_cancelled() -> bool {
    CANCEL.load(Ordering::Relaxed)
}
