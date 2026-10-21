//! Process-wide solver-phase progress reporting.
//!
//! The push-dual of [`super::cancel`]: where cancellation is a flag the search
//! loops *poll*, progress is a sink the search loops *push* phase transitions
//! to. Both rely on the same invariant — the CLI and TUI run at most one solve
//! at a time — so a single global slot suffices. A caller that wants live phase
//! updates installs a sink with [`set_sink`] before starting a solve; whoever
//! starts a fresh job calls [`clear`] first (the TUI job runner does this,
//! alongside `cancel::clear`). The CLI installs no sink, so [`emit`] is a cheap
//! no-op there. Emit points sit at coarse phase boundaries on the solve's
//! driver thread, never inside the hot search loops.

use std::sync::Mutex;

use super::SolverPhase;

type Sink = Box<dyn Fn(SolverPhase) + Send + Sync>;

static SINK: Mutex<Option<Sink>> = Mutex::new(None);

/// Install the phase sink for the in-flight solve. Call before starting.
pub fn set_sink(sink: Sink) {
    *SINK.lock().unwrap() = Some(sink);
}

/// Remove the sink. Call before starting a new job so no solve emits into a
/// previous job's channel.
pub fn clear() {
    *SINK.lock().unwrap() = None;
}

/// Report that the solve has entered `phase`. A no-op when no sink is installed.
pub(crate) fn emit(phase: SolverPhase) {
    if let Some(sink) = SINK.lock().unwrap().as_ref() {
        sink(phase);
    }
}
