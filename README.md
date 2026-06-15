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

---

## Requirements

- A Starsector installation (the tool auto-detects common install locations, or
  you can point it with `--starsector-dir` / the `STARSECTOR_DIR` env var).
- Nothing else - the tool is a self-contained native binary and the installer is
  built in (no Python or other runtime required).
- For building from source: a [Rust toolchain](https://rustup.rs/) (1.74+).

## Install (recommended)

Grab the archive for your OS from the
[latest release](https://github.com/0neye/starsector_system_solver/releases),
unpack it, and run the bundled installer from inside the unpacked folder. The
installer is built into the binary; `install.ps1` / `install.sh` are thin
wrappers around `system_solver install` (equivalently, run that directly).

**Windows**: in the unpacked `starsector-system-ranker-*-windows` folder,
right-click `install.ps1` and choose **Run with PowerShell** (or run it from a
PowerShell prompt). It walks you through the rest and keeps the window open when
it finishes.

```powershell
.\install.ps1
```

**Linux** (in the unpacked `starsector-system-ranker-*-linux` folder):

```bash
./install.sh
```

The installer copies `system_solver` to a per-user location (no admin rights
needed), adds it to your `PATH`, runs an initial save extraction, and creates a
launcher (Start Menu shortcut on Windows, `.desktop` entry on Linux). Open a new
terminal afterwards, then run `system_solver tui`.

Useful installer flags (pass them to `install.ps1` / `install.sh`):

| Flag | Effect |
|------|--------|
| `--starsector-dir <PATH>` | Point at your Starsector install instead of auto-detecting |
| `--yes` | Non-interactive install (auto-detect everything) |
| `--skip-extract` | Don't run the initial save extraction |
| `--no-shortcut` | Don't create a launcher |
| `--with-skills` / `--no-skills` | Install / skip the bundled Claude Code & Codex agent skill |

To upgrade, unpack a newer release and run the installer again. To remove
everything, run `system_solver uninstall` (your extracted data and settings are
left untouched).

## Build from source

If you'd rather not use the release archive:

```bash
git clone https://github.com/0neye/starsector_system_solver
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

Subcommands: `extract` (parse/search/export saves), `locate` (find the game
install), `init` (save settings and build the first DB), `inspect` (dump
extracted system data), `tui` (interactive UI). Run `system_solver --help` or
`system_solver <subcommand> --help`; see [`docs/CLI.md`](docs/CLI.md) for the
full CLI reference and workflow examples.

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
[`AGENTS.md`](AGENTS.md) - these two files are kept in sync.

## License

MIT - see [`LICENSE`](LICENSE).
