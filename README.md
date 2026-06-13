# Starsector System Ranker

A colony-planning solver and ranker for [Starsector](https://fractalsoftworks.com/).
Point it at your save and it will:

- **Rank** every star system you've discovered by how much economic potential it
  has for a player colony.
- **Solve** a chosen system for a near time-optimal sequence of colony actions
  (colonize, build facilities, install items/cores, set free port / hazard pay)
  to reach an income / stability / defense goal in the fewest game months.
- Do it all from a terminal **TUI** or a scriptable **CLI**.

It reads your local game files and save data only; it never modifies your game.

> **Disclaimer:** This is an unofficial fan-made tool. It is not affiliated with,
> endorsed by, or supported by Fractal Softworks. You must own a copy of
> Starsector to use it — the tool reads the game's data files and your saves
> from your own machine. "Starsector" is a trademark of Fractal Softworks.

---

## Requirements

- A Starsector installation (the tool auto-detects common install locations, or
  you can point it with `--starsector-dir` / the `STARSECTOR_DIR` env var).
- For building from source: a [Rust toolchain](https://rustup.rs/) (1.74+).

> A downloadable release with a one-click installer (Windows + Linux) is planned.
> Until then, build from source as below.

## Build from source

```bash
git clone <this-repo>
cd system_solver
cargo build --release
# binary at target/release/system_solver(.exe)
```

SQLite is statically bundled (via `rusqlite`), so there is no system database
dependency.

## Quickstart

```bash
# 1. Extract a save into the local database (auto-detects your install).
#    Use --latest for the most recent save, or --save <substring> to pick one.
system_solver extract run --latest

# 2. Rank your discovered systems by colony potential.
system_solver --rank --rank-scope discovered

# 3. Solve a specific system for a balanced plan.
system_solver --solve --system "Mia's Star"

# 4. Or drive everything from the interactive terminal UI.
system_solver tui
```

The extraction database and your settings are stored in a per-user location
(e.g. `%APPDATA%\StarsectorSystemRanker` on Windows,
`~/.local/share/StarsectorSystemRanker` and `~/.config/StarsectorSystemRanker`
on Linux), so the commands above work from any directory once you've extracted a
save. Pass `--db <path>` to override the database location.

## Common CLI options

| Flag | Meaning |
|------|---------|
| `--system <NAME>` | System to solve (as named in your save) |
| `--income <N>` / `--stability <N>` / `--defense <N>` | Goal thresholds |
| `--solve` | Pareto solve + recommended balanced plan |
| `--maximize <income\|stability\|defense>` | Maximize one metric within a horizon, holding the others as floors |
| `--rank` | Score and rank systems best-first |
| `--rank-scope discovered` | Limit ranking to systems you've surveyed |
| `--credits` / `--story-points` / `--alpha-cores` / `--item <NAME>` | Starting resources |
| `--time-limit <MS>` | Hard wall-clock budget for the solve |
| `--no-industry-upgrades` | Disable story-point improvements and alpha-core installs |

Subcommands: `extract` (parse/search/export saves), `inspect` (dump extracted
system data), `tui` (interactive UI). Run `system_solver --help` or
`system_solver <subcommand> --help` for the full reference.

## How it works

The solver factors the search into a plan level (per-planet decisions) and a
schedule level (a greedy build-ASAP simulation that models shared resource
contention across planets). Colony export income is pooled at system scope, so
duplicate industries see diminishing returns. See [`CLAUDE.md`](CLAUDE.md) for
the architecture and invariants.

## Development

```bash
cargo build
cargo test
cargo clippy
```

Tests live in `src/tests/` (in-crate, so they can exercise `pub(crate)`
internals). Contributor/agent guidance lives in [`CLAUDE.md`](CLAUDE.md) and
[`AGENTS.md`](AGENTS.md) — these two files are kept in sync.

## License

MIT — see [`LICENSE`](LICENSE).
