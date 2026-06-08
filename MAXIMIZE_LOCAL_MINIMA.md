# Maximize-mode local minima: diagnosis and fix options

Status: **resolved** — Tier 1/2 fix implemented in `src/solver/decomp.rs`. The
sections below are the original design note; see **Resolution** immediately after
the TL;DR for what shipped and the measured outcome.

## Resolution (implemented)

A best-improvement Variable-Neighborhood-Descent replaced the old drop-only,
ordered first-improvement climb:

- **`one_move_neighbors`** generates the full *bidirectional* one-move
  neighborhood — drop **or add** a facility (adds pull in their prerequisite
  upgrade chain via `upgrade_predecessors`), and flip any non-`Colonize` toggle
  either way. `Colonize` stays owned by the `choose_planet_set` seed.
- **`hill_climb`** evaluates the whole fresh neighborhood each pass and applies
  the single steepest improving move, then restarts (deterministic, sorted order).
- **Swaps** (`swap_neighbors`, drop A + add B) were built but left **off** in the
  production climb: the diagnostic showed single moves already clear the gap and
  swaps add nothing from that optimum.

Diagnostic harness: `SYSTEM_SOLVER_DIAG=<system> cargo run` runs
`diagnose_maximize_gap`, reporting seed value, single-move-VND optimum, and the
best single/swap step from it.

Measured outcome (`Mia Bravos`, `--maximize income --stability 6`, horizon 120):

| Search | Income |
|--------|-------:|
| old drop-only first-improvement | 277797 |
| better basin it missed (cited)  | 281547 |
| **bidirectional best-improvement VND** | **303737** |

Best swap step from the VND optimum: **+0**. Floors held (stability 6.0); the run
is deterministic. The same change also *improved reach mode* (it shares the
climb): VERIFY makespans dropped 6mo→2mo (Epsilon Gamma) and 14mo→7mo
(Mia Bravos), with every system still solved, satisfied, and resource-valid.

Regression fixture: `decomp_maximize_mia_bravos_escapes_local_optimum`
(`src/tests/solver.rs`) loads the real game data, pins income > 290000, holds the
floor, and asserts run-to-run determinism.

Not done (deferred, as the note recommends): multi-start, ILS, and all Tier 3
structural reframes — unnecessary once single-move VND cleared the gap.

---

*Original design note follows.*

Status: design note (no implementation yet). Context for the maximize objective
added to the two-level decomposition solver (`src/solver/decomp.rs`).

## TL;DR

The maximize search converges to a **local optimum**, not the global one — e.g.
on `Mia Bravos`, `--maximize income --stability 6` settles on income **277797**
when a strictly better **281547** plan exists in the same space. The cause is the
*shape* of the outer search (a one-way, single-move greedy prune), not its
compute budget: it converges at **57 evaluated plans** with `cutoff_occurred:
false`, using a sub-millisecond sliver of the multi-second time limit. **Compute
is effectively free**, so almost any richer search is affordable.

(Run-to-run nondeterminism that previously masked this — different optima on
different runs — was a separate bug, fixed in `41ba6ad` by sorting the climb's
candidate order.)

## How the outer search works today

`decomp_search_objective` (`src/solver/decomp.rs`):

1. **Seed** — `choose_planet_set` decides *which* planets to develop and seeds a
   maximal per-planet plan (every facility + every toggle, free port per
   `seed_free_ports`).
2. **Hill-climb** — repeatedly try **dropping** one facility, **turning off** one
   toggle, or **flipping** free port; accept the first move that
   `is_better` approves; stop when no single move improves.

Ranking (`is_better`) in maximize mode: higher metric value, then fewer months,
then smaller plan. Floors are hard constraints; infeasible plans are ranked by
violation so the climb can descend toward feasibility.

## Why it gets stuck

1. **Drop-only neighborhood.** From the maximal seed the climb almost only
   *removes* structure (only free port is bidirectional). It is a monotone prune.
2. **First-improvement, single-element moves.** It stops at the first plan where
   no *single* removal helps — even when a *pair* of removals, or a
   remove-then-add, would.
3. **The landscape is genuinely rugged.** Level-2 is budget/time-constrained:
   within a horizon you cannot build everything, so **dropping a facility can
   *raise* the metric** by freeing credits/time to build something more valuable
   sooner. Value is therefore **not monotone in plan-inclusion** — the worst
   possible shape for a drop-only descent. The better basin (281547) sits one
   ridge over from the one the climb reaches (277797).

Key consequence: more search, not more simulation budget, is what's missing.

## Options

Ordered roughly by value-per-effort. Effort is relative (S/M/L); all Tier 1–2
options are small because the node budget is wide open.

### Tier 1 — spend the idle budget

#### 1a. Multi-start (VND-style restarts) — **effort: S**
Run the existing climb from several *diverse, deterministic* seeds and keep the
best:
- maximal + free-port-on, maximal + free-port-off (already seeds at the
  planet-set level),
- **empty + greedy-add** (opposite construction direction),
- "metric-helping facilities only."

Different seeds fall into different basins; keeping the best routinely escapes
traps like 277797 → 281547. At ~57 nodes per descent we can afford dozens of
restarts and still finish in milliseconds.

- **Pros:** tiny, deterministic, immediate robustness win.
- **Cons:** still bounded by whatever basins the seeds happen to touch.

#### 1b. Iterated Local Search (ILS) — **effort: S/M**
On convergence, **perturb** the incumbent (flip *k* decisions via a fixed-seed
PRNG) and re-descend; keep the global best; repeat until a set fraction of the
time budget is used.

- **Pros:** canonical, compact escape mechanism; converts the unused seconds
  directly into quality; tunable via *k* and restart count.
- **Cons:** introduces a (seeded) RNG; needs a stopping rule.

### Tier 2 — fix the neighborhood shape

#### 2a. Bidirectional moves — **effort: S/M**
Allow *add-facility* and *toggle-on* moves, not just drops. The search then walks
the full decision hypercube instead of pruning one way, and can climb back up
where the budget tradeoff rewards it.

- **Pros:** directly addresses cause #1; composes with multi-start/ILS.
- **Cons:** larger neighborhood per step (still cheap); must guard against
  re-adding a facility whose prerequisite was dropped (legality already enforced
  by the generator, so mostly a no-op cost).

#### 2b. Swap moves (drop A + add B) — **effort: M**
A single move that expresses the credit/time tradeoff the rugged landscape is
made of; crosses basins that two independent single flips cannot.

- **Pros:** targets cause #3 head-on.
- **Cons:** quadratic neighborhood (pairs); fine at this scale but the biggest of
  the Tier 2 moves.

#### 2c. Best-improvement instead of first-improvement — **effort: S**
Evaluate the whole neighborhood and take the steepest move.

- **Pros:** removes path-dependence; tends toward better optima; trivial.
- **Cons:** does not by itself escape a true local optimum (pair with 1/2).

### Tier 3 — structural reframes (more elegant, bigger lift)

#### 3a. Value-aware Level-2 scheduler — **effort: M/L**
Today Level-2 builds in a fixed action priority. Build instead in
**marginal-metric-per-(cost, build-time)** order, so the scheduler itself spends
the budget near-optimally and the *plan* search barely matters.

- **Pros:** attacks the root cause (schedule-dependent ruggedness) rather than
  searching around it; likely improves *every* mode, not just maximize.
- **Cons:** changes Level-2 semantics; must keep the reach-mode contract intact;
  "marginal value" is itself horizon-dependent.

#### 3b. Budget decomposition / DP — **effort: L**
Planets couple essentially only through the shared credit/SP/core pool and the
single timeline. Solve each planet's best metric for a range of budget
allocations, then combine with a knapsack/DP over the shared pool.

- **Pros:** turns a rugged joint search into independent subproblems + a clean
  combine; principled.
- **Cons:** largest change; the shared *timeline* (not just credits) makes the
  coupling not perfectly separable — needs care.

#### 3c. Branch-and-bound over the per-planet decision set — **effort: M/L**
The per-planet decision set is small (a handful of facilities + 7 toggles). With
an admissible upper bound (e.g., metric if all plan facilities were free and
instant), bound-and-prune could be near-exact.

- **Pros:** can **prove** optimality instead of hoping; we currently explore only
  57 nodes, so there is enormous headroom.
- **Cons:** needs a genuinely admissible, reasonably tight bound; worst-case
  exponential if the bound is loose.

## Recommendation

Start with **Tier 1 (multi-start) + Tier 2a/2c (bidirectional + best-improvement)**
— a small Variable-Neighborhood-Descent. It is tiny, fully deterministic, fixes
both the path-dependence and the wrong-shaped neighborhood, and finally *uses* the
budget that currently sits idle. Validate it against the known 281547 case before
expanding.

Treat the **value-aware Level-2 scheduler (3a)** as the elegant longer-term
structural fix; it pairs naturally with the VND outer loop and should lift quality
across all modes.

## Validation hooks

- Reproduce the gap: `--maximize income --stability 6` on `Mia Bravos` should find
  **281547**, not 277797, once a fix lands.
- Determinism: repeated identical runs must return identical income/defense
  (regression on `41ba6ad`).
- Per-plan monotonicity in the horizon is covered by
  `decomp_maximize_longer_horizon_is_no_worse_for_fixed_plan`
  (`src/tests/solver.rs`) via `simulate_plan_maximize`.
- Watch `nodes_searched` / `cutoff_occurred` to confirm a richer search is
  actually exploring more and still terminating within budget.

## Critique / implementation notes

The core diagnosis is sound: the current outer loop seeds from a maximal plan and
mostly prunes downward. That is the wrong neighborhood shape for maximize mode,
where dropping one build target can improve the schedule by freeing credits/time,
but the better plan may still require adding or swapping another target later.

One terminology correction: the climb is not quite "first-improvement" in the
strict sense, because after accepting an improving move it keeps scanning the
remaining precomputed candidates in that pass rather than immediately restarting
from a fresh neighborhood. It is still ordered, path-dependent greedy
improvement, so best-improvement remains useful; the option should be read as
"evaluate the full fresh neighborhood, apply the best single move, then repeat."

Recommended implementation order:

1. **Best-improvement VND first.** This is the smallest behavioral cleanup and
   makes subsequent comparisons easier to reason about. Evaluate every legal
   one-move neighbor from the current plan, take the best, and restart.
2. **Bidirectional moves next.** This directly fixes the drop-only shape. The
   main caveat is prerequisite closure: adding `Megaport` without permitting
   `Spaceport`, or adding an upper station tier without its chain, can create a
   dead plan even though the action generator remains legal. Add moves should
   either include required predecessor facilities or use a helper that normalizes
   a plan to include prerequisite chains.
3. **Multi-start after add/remove helpers exist.** Empty + greedy-add and
   metric-helping seeds both depend on the same legality/prerequisite machinery,
   so implementing them after bidirectional moves avoids duplicating logic.
4. **Swap moves after bidirectional moves.** Swaps are probably the strongest
   escape hatch for the observed credit/time tradeoff, but they should reuse the
   same add/remove/prerequisite helpers.
5. **ILS later, if needed.** A fixed-seed perturbation loop is reasonable, but
   deterministic VND should be exhausted first because this search space is
   currently tiny and easier to regression-test without RNG tuning.

Tier 3 should be treated carefully. A value-aware Level-2 scheduler is attractive
but changes the meaning of `run_plan`: today it takes the first allowed
non-wait action from `get_ordered_possible_actions`, whose priority order is
shared with the rest of the solver. A scheduler rewrite should probably be
objective-specific or guarded by strong reach-mode regression tests. The DP and
branch-and-bound ideas are principled, but the shared timeline, colony growth,
free-port maturation, global credits, and facility prerequisites make clean
decomposition or tight admissible bounds harder than the small plan-search fixes.

Validation should also be strengthened. The existing maximize tests mainly check
that floors are held and that fixed-plan horizon monotonicity works; they do not
pin the outer search quality. The `Mia Bravos` `--maximize income --stability 6`
case should become a regression fixture once the expected `281547` plan is
reproducible.
