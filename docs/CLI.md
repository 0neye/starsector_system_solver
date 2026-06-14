# System Solver CLI Reference

`system_solver` is a command-line solver and ranker for Starsector colony
planning. It reads your local Starsector install and save files, writes an
extraction database, and then uses that database for ranking and solving.

All solver modes read from the extraction DB built by `system_solver init` or
`system_solver extract run`. By default the DB and TUI config live in per-user
locations resolved by `src/paths.rs`, with development fallbacks to local files.
Use `--db <path>` for one command, or `SYSTEM_SOLVER_DB`, to override the DB.

## Typical workflow

```bash
# One-command setup: locate Starsector, save settings, and extract the newest save.
system_solver init

# Equivalent manual extraction path.
system_solver extract run --latest

# Shortlist promising surveyed systems.
system_solver --rank --rank-scope discovered

# Solve one system and print Pareto frontiers plus a recommended tradeoff.
system_solver --solve --system "Mia's Star"

# Open the interactive UI.
system_solver tui
```

Use `system_solver extract search <query>` to find the exact saved system name
before solving. Use `--save <substring>` when the DB contains multiple extracted
saves and you want a specific one.

## Top-level command

```text
system_solver [OPTIONS] [COMMAND]
```

Commands:

- `extract` - save-game extraction tools.
- `locate` - print the Starsector install and saves directories.
- `init` - save the Starsector install path and build the first DB.
- `inspect` - inspect extracted system data.
- `tui` - open the terminal UI.

### Data selection

- `--system <SYSTEM>`: star system to solve. Default: `"Mia's Star"`.
- `--db <DB>`: extraction DB to load. Default: per-user DB, falling back to
  `save_data.db`.
- `--save <SAVE>`: extracted save substring or character name. Defaults to the
  most recently extracted save.

### Reach-goal, solve, and maximize modes

Without `--rank`, `--solve`, or `--maximize`, the CLI searches for a sequence of
actions that reaches the requested goal in minimum time.

- `--income <INCOME>`: minimum net income. Default: `200000` in reach mode and
  `0` as a `--maximize` floor.
- `--stability <STABILITY>`: minimum average stability.
- `--defense <DEFENSE>`: minimum average ground defense.
- `--solve`: score the selected system with Pareto frontiers and recommend a
  balanced plan.
- `--maximize <income|defense|stability>`: maximize one metric within the
  horizon while holding the other requested metrics as floors.
- `--horizon <HORIZON>`: game-month horizon for `--maximize`. Default: `120`.

`--solve` output includes stability and defense frontier points, then a
recommended balanced tradeoff. Treat the frontiers as a map of possible
income-vs-safety choices; the recommendation is a convenience pick, not the only
valid colony plan.

### Starting resources and solver behavior

- `--credits <CREDITS>`: starting credits. Default: `5000000`.
- `--story-points <STORY_POINTS>`: starting story points. Default: `5`.
- `--alpha-cores <ALPHA_CORES>`: starting alpha cores. Default: `1`.
- `--item <ITEMS>`: starting colony item. Repeat for multiple items.
- `--no-industry-upgrades`: disable story-point improvements and industry or
  structure alpha-core installs. They are included by default.
- `--parallel-builds`: allow multiple industries or structures to build at once
  on the same colony. By default, vanilla one-at-a-time colony queueing is
  enforced.
- `--time-limit <TIME_LIMIT>`: hard wall-clock solver budget in milliseconds.
  Default: `25000`.

If the time limit fires, the solver returns its best-so-far plan. Results are
deterministic only when the solve finishes inside the budget.

### Ranking

- `--rank`: rank systems by quick Pareto score. Ignores `--system`.
- `--rank-system <RANK_SYSTEMS>`: repeatable case-insensitive substring filter.
- `--rank-scorer <quick|template|bound>`: scorer. Default: `quick`.
- `--rank-scope <all|discovered>`: systems considered before name filters.
  Default: `all`.
- `--discovery-definition <at-least-one-surveyed|fully-surveyed>`: discovery
  rule for `--rank-scope discovered`. Default: `at-least-one-surveyed`.
- `--include-core-worlds`: include core-world systems with discovered ranking.
- `--rank-sort <score-per-planet|total-score>`: rank sorting mode. Default:
  `score-per-planet`.
- `--rank-csv`: emit `system,score,peak_income,seconds` CSV instead of the
  human-readable table.

Ranking scores are deterministic orderings for shortlisting systems. They are
not final colony values; use `--solve --system <NAME>` on finalists.

Scorers:

- `quick`: budgeted real search and the default ranking scorer.
- `template`: instant template portfolio, typically a rough lower bound.
- `bound`: credit-relaxed per-planet potential ceiling.

## `extract`

```text
system_solver extract <COMMAND>
```

Subcommands:

- `list-saves`
- `run`
- `search`
- `export`

### `extract list-saves`

```text
system_solver extract list-saves [OPTIONS]
```

- `--saves-dir <SAVES_DIR>`: saves directory. Defaults to
  `<starsector-dir>/saves` from auto-detection.

### `extract run`

```text
system_solver extract run [OPTIONS]
```

Parses a save's `campaign.xml` and writes systems, planets, infrastructure, and
player balance data to the DB.

- `--saves-dir <SAVES_DIR>`: saves directory. Defaults to
  `<starsector-dir>/saves`.
- `--save <SAVE>`: save substring or character name to extract.
- `--latest`: extract the newest save.
- `--starsector-dir <STARSECTOR_DIR>`: Starsector install directory. When
  omitted, auto-detected from `STARSECTOR_DIR` or common install locations.
- `--db <DB>`: output DB. Default: per-user DB, falling back to `save_data.db`.
- `--system <SYSTEM>`: repeatable system filter.

### `extract search`

```text
system_solver extract search [OPTIONS] <QUERY>
```

- `<QUERY>`: system search text.
- `--db <DB>`: DB to search. Default: per-user DB, falling back to
  `save_data.db`.
- `--save <SAVE>`: save substring or character name.
- `--limit <LIMIT>`: maximum results. Default: `20`.

### `extract export`

```text
system_solver extract export [OPTIONS]
```

- `--db <DB>`: DB to export. Default: per-user DB, falling back to
  `save_data.db`.
- `--save <SAVE>`: save substring or character name.
- `--out-dir <OUT_DIR>`: output directory. Default: `out`.
- `--system <SYSTEM>`: repeatable system filter.

## `locate`

```text
system_solver locate [OPTIONS]
```

- `--starsector-dir <STARSECTOR_DIR>`: explicit install directory. When omitted,
  auto-detected from `STARSECTOR_DIR` or common install locations.

On success, prints the resolved Starsector directory and the saves directory.

## `init`

```text
system_solver init [OPTIONS]
```

Saves the Starsector install path to the TUI config and builds the first
extraction DB. When both `--save` and `--latest` are omitted, `init` uses
`--latest` by default so installer setup works without extra flags.

- `--starsector-dir <STARSECTOR_DIR>`: explicit install directory. When omitted,
  auto-detected from `STARSECTOR_DIR` or common install locations.
- `--save <SAVE>`: save substring to extract.
- `--latest`: extract the newest save.
- `--db <DB>`: output DB. Default: per-user DB.

## `inspect`

```text
system_solver inspect [OPTIONS] [SYSTEM]...
```

- `[SYSTEM]...`: system name substring filters.
- `--db <DB>`: extraction DB to inspect. Default: per-user DB, falling back to
  `save_data.db`.
- `--save <SAVE>`: extracted save to inspect. Defaults to the most recently
  extracted save.
- `--all`: show every system in the save.

## `tui`

```text
system_solver tui [OPTIONS]
```

- `--starsector-dir <STARSECTOR_DIR>`: Starsector install directory for this
  run, overriding the saved TUI config.
