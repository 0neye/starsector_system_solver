# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build
cargo test
cargo clippy
cargo run -- --system "Mia's Star" --income 200000 --stability 8 --time-limit 25000

# Interactive TUI (see TUI_DESIGN.md): Saves -> Rank -> System -> Solve -> Plan,
# with a discovered-only scope filter (>=1 surveyed planet, core worlds excluded
# by default) and settings persisted to solver_tui.toml. Rank/extract/solve run
# as background jobs; x cooperatively cancels rank/solve (solver::cancel flag),
# extract/load only detach. Loading a save's systems seeds the Setup balance
# from the save (credits/SP/alpha cores/colony items; player_balance table).
cargo run --release -- tui

# A/B test decomp vs IDA* across all systems/goals
SYSTEM_SOLVER_AB=1 cargo run
SYSTEM_SOLVER_AB_MS=8000 cargo run   # custom time budget (ms)

# Replay solutions on fresh state to verify correctness
SYSTEM_SOLVER_VERIFY=1 cargo run

# Sample Pareto-frontier data (income vs stability/defense) as CSV, then plot it
SYSTEM_SOLVER_PARETO=1 cargo run
python plot_pareto_frontiers.py   # writes pareto_frontiers.png
# Pareto sweep extras:
#   SYSTEM_SOLVER_PARETO_SYSTEM=<substring>  limit the sweep to one system
#   SYSTEM_SOLVER_STATS=1                    per-point timings + per-system search counters (stderr)
#   SYSTEM_SOLVER_QUALITY=1                  slow max-quality reference config (full climb
#                                            refresh; used to gate speed optimizations)
#   SYSTEM_SOLVER_NO_UPGRADES=1              disable SP improvements / alpha-core installs
#                                            (matches --no-industry-upgrades; also honored
#                                            by the bound sweep)
# Benchmark workflow (see SOLVER_OPTIMIZATION_REPORT.md): run a sweep against
# system_benchmark.db, then `python compare_pareto.py <ref.csv> <cand.csv> [tol_pct]`
# (exit 1 on income regression).

# Measure greedy income vs a credit-relaxed upper bound (how much headroom the
# greedy leaves on the credit-timing axis). CSV to stdout, summary to stderr.
SYSTEM_SOLVER_BOUND=1 cargo run
SYSTEM_SOLVER_BOUND_MS=5000 cargo run   # budget per point. See OPTIMAL_SOLVER_BOUND.md
#   SYSTEM_SOLVER_BOUND_SYSTEM=<substring>  limit the bound sweep to one system
# The bound sweep warm-chains floors and cross-seeds each relaxed solve with the
# greedy plan, so bound >= greedy by construction (negative gaps = solver bug).
```

Key CLI flags: `--income`, `--stability`, `--defense`, `--credits`, `--story-points`, `--alpha-cores`, `--item <NAME>` (repeatable), `--time-limit <MS>`, `--no-industry-upgrades`
(SP improvements and industry/structure alpha cores are searched by default).

`--time-limit` is a hard wall-clock deadline: the decomp climbs poll it (and the
cooperative-cancel flag, `solver::cancel`) at node granularity and return their
best-so-far plan when it fires. Results are deterministic only when the solve
finishes inside the budget; a cutoff is machine-dependent and reported via
`cutoff_occurred`.

```bash
# Quick ranking: score every system with a reduced deterministic sweep (sparse
# floors + mini-anchor repair climbs with a feasibility bridge; see
# QUICK_RANKING_DESIGN.md), print best-first.
# SYSTEM_SOLVER_RANK_POINTS=1: per-point income/months/profile trace (stderr).
cargo run --release -- --db system_benchmark.db --rank
cargo run --release -- --rank --rank-system askonia --rank-system corvus  # filter
cargo run --release -- --rank --rank-csv > rank.csv  # machine-readable
python rank_validation.py final_sweep.csv rank.csv   # rank-agreement gate vs full sweep
# --rank-scorer picks the scorer (default `quick` = Tier-1 budgeted search):
#   bound    = floor-aware per-planet credit-relaxed upper bound
#              (solve_pareto_bound): floor-0 one-shot rationing, per-floor
#              menus, integer average-floor DP, and flat-left AUC. Near-certain
#              ceiling, not certified: rationing is exact only under concavity
#              and is fixed at floor 0. Validate: bound/full should be >= 1.
#   template = instant template portfolio, no search, in practice a lower bound
#              (solve_pareto_template, ~ms/system, rougher ordering).
cargo run --release -- --rank --rank-scorer bound
cargo run --release -- --rank --rank-scorer template
```

### Game data source

All solver modes (CLI and env-var modes) load game data from the save-extraction
SQLite DB (`save_data.db` by default, built by `extract run`), via
`parser::load_game_data_from_db`. Select the DB/save with `--db <path>` /
`--save <substring>` on the CLI, or `SYSTEM_SOLVER_DB` / `SYSTEM_SOLVER_SAVE`
for the env-var modes (default: most recently extracted save). The CSV parser
(`parser::load_game_data`) is kept for tests and for consuming `extract export`
output; the DB loader is verified to match its semantics exactly
(`db_loader_matches_csv_semantics` in `src/tests/parser.rs`).

```bash
# Save-game extraction (see SAVE_EXTRACTION_DESIGN.md): parses a Starsector save
# into save_data.db with tables mirroring the three CSVs. Available as a
# subcommand of the main CLI:
cargo run --release -- extract list-saves
cargo run --release -- extract run --save DEMIURGE --latest
cargo run --release -- extract search "askonia"
cargo run --release -- extract export --system Corvus --out-dir out/extract_test
```

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
| `parser/mod.rs` | — | `load_game_data_from_db()` (primary) + `load_game_data()` CSV fallback |

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
