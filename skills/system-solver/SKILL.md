---
name: system-solver
description: Use when working with Starsector colony planning, colony or star-system ranking, or the system_solver CLI for extracting saves, ranking systems, solving colony plans, or maximizing Starsector colony outcomes.
---

# System Solver

Use the `system_solver` CLI to extract Starsector save data, rank candidate
systems, and solve colony plans.

## Prerequisites

- `system_solver` must be on PATH. Check with `system_solver --help`.
- A DB must exist, built by `system_solver init` or `system_solver extract run`.
- If the DB is missing, the CLI reports an error like:
  `extraction DB <path> not found` and tells you to run `extract run` first.

Fast setup:

```bash
system_solver init
```

Manual setup:

```bash
system_solver locate
system_solver extract run --latest
```

Use `--starsector-dir <path>` when auto-detection cannot find the game install.
Use `--db <path>` or `SYSTEM_SOLVER_DB` for a non-default extraction DB.

## Workflow

1. Extract the save:

```bash
system_solver extract run --latest
```

2. Rank surveyed systems:

```bash
system_solver --rank --rank-scope discovered
```

3. Solve a finalist:

```bash
system_solver --solve --system "Mia's Star"
```

4. Optionally maximize one metric:

```bash
system_solver --maximize income --system "Mia's Star" --stability 8 --horizon 120
```

Use `system_solver extract search <query>` when you need the exact system name
stored in the save.

## Key Flags

- Goals: `--income`, `--stability`, `--defense`.
- Starting balance: `--credits`, `--story-points`, `--alpha-cores`,
  repeatable `--item <NAME>`.
- Solver budget: `--time-limit <MS>`.
- Save/DB selection: `--save <substring>`, `--db <path>`.
- Vanilla queueing is default; `--parallel-builds` enables modded same-colony
  parallel builds.
- Industry improvements and alpha-core installs are searched by default;
  `--no-industry-upgrades` disables them.

Ranking:

- `--rank-scope discovered` limits to discovered systems.
- `--discovery-definition at-least-one-surveyed|fully-surveyed` controls what
  discovered means.
- `--rank-scorer quick|bound|template` selects the ranking scorer.
- `--rank-sort score-per-planet|total-score` selects the ordering shape.
- `--rank-csv` emits machine-readable rows.

Scorer notes:

- `quick`: default, budgeted real search.
- `bound`: credit-relaxed potential ceiling.
- `template`: instant rough lower bound.

## Interpreting Output

- `--rank` scores are deterministic ordering aids for shortlisting. Do not treat
  them as final colony values.
- `--solve` prints stability and defense frontiers, then a recommended balanced
  tradeoff. Use the frontier points to choose between income and safety.
- Results are deterministic only when the solve finishes within `--time-limit`;
  a cutoff returns best-so-far and is machine-dependent.

For the complete reference, read `docs/CLI.md` in the repository or release
archive.
