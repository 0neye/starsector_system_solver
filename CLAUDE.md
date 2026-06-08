# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build
cargo test
cargo clippy
cargo run -- --system "Mia Bravos" --income 200000 --stability 8 --time-limit 25000

# A/B test decomp vs IDA* across all systems/goals
SYSTEM_SOLVER_AB=1 cargo run
SYSTEM_SOLVER_AB_MS=8000 cargo run   # custom time budget (ms)

# Replay solutions on fresh state to verify correctness
SYSTEM_SOLVER_VERIFY=1 cargo run

# Sample Pareto-frontier data (income vs stability/defense) as CSV, then plot it
SYSTEM_SOLVER_PARETO=1 cargo run
python plot_pareto_frontiers.py   # writes pareto_frontiers.png
```

Key CLI flags: `--income`, `--stability`, `--defense`, `--credits`, `--story-points`, `--alpha-cores`, `--item <NAME>` (repeatable), `--time-limit <MS>`.

## Architecture

This solver finds the minimum-time (months) sequence of colony actions to reach an income/stability/defense goal in Starsector. The search space is factored into two levels:

**Level 1 — Plan search** (`solver/decomp.rs`): Breadth-first over `SystemPlan`, which encodes per-planet decisions (colonize? free port? hazard pay? which facilities/improvements/cores/items?).

**Level 2 — Schedule simulation** (`solver/decomp.rs:simulate_plan`): Given a fixed plan, uses a greedy "build ASAP, wait minimum interval" loop. A single `Wait(n)` advances all planets concurrently, correctly modeling shared resource contention (credits, story points, alpha cores).

The archived solvers (`solver/archive/`) — IDA\* per-planet and Rayon-parallel split — are kept for A/B comparison only.

### Key types

| File | Type | Role |
|------|------|------|
| `solver/state.rs` | `State`, `Balance`, `Action` | Full mutable search state + reversible actions |
| `planet/mod.rs` | `Planet` | Per-colony mechanics (size, growth, stability, income) |
| `planet/facility.rs` | `Facility` | Build progress, improvements, alpha cores, installed items |
| `system/mod.rs` | `System` | Multi-planet aggregate (avg stability, defense, net income) |
| `solver/goal.rs` | `Goal` | Threshold triple; `goal.is_satisfied(&state)` |
| `constants.rs` | `FACILITY_DATA`, etc. | `lazy_static!` game data maps |
| `parser/mod.rs` | — | `load_game_data()` from Planets.csv + Infrastructure.csv |

### Reversible-action invariant

Every `Action` variant is undoable: `state.apply_action_raw(&action, verbose)` / `state.undo_last_action(verbose)`. This lets tree search avoid full state cloning. The round-trip must leave state byte-for-byte identical — `_test_path_undo_consistency()` in `solver.rs` tests enforces this. If you add a new action variant, maintain this symmetry exactly.

### Test suite

Tests live in `src/tests/` (not `tests/`) because they exercise `pub(crate)` internals.

- `support.rs` — `PlanetBuilder`, `colonized_state()`, `apply_all()` fixtures
- `planet.rs`, `facility.rs` — game mechanics
- `solver.rs` — action hashing and apply/undo invariants
- `system.rs` — system-wide aggregates

### Planet keying

Planets are stored in `HashMap<u64, Planet>` keyed by name hash (`Planet::_get_planet_name_hash`), using `nohash-hasher` (identity hash for pre-hashed u64 keys). `rustc-hash` (FxHasher) is used for facility lookups and action sequence hashing.

### Adding a new facility type

1. Add variant to `FacilityType` in `constants.rs` and update `as_str()`/`from_str()`.
2. Add entry to `FACILITY_DATA` with cost, build time, and production/demand functions.
3. Update Planets.csv if any planet should start with it.
