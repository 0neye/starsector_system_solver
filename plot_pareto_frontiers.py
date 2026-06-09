"""Plot Pareto frontiers from solver CSV output.

Generate the CSV with:
    SYSTEM_SOLVER_PARETO=1 cargo run > data.csv

Then plot:
    python plot_pareto_frontiers.py data.csv --output pareto_frontiers.png

Or pipe directly (Windows — write to a temp file first, since cargo mixes
stderr into stdout on a direct pipe):
    SYSTEM_SOLVER_PARETO=1 cargo run > data.csv && python plot_pareto_frontiers.py data.csv

Customise the starting resources:
    SYSTEM_SOLVER_PARETO=1 SYSTEM_SOLVER_PARETO_SP=1000 ^
        SYSTEM_SOLVER_PARETO_ALPHA=100 SYSTEM_SOLVER_PARETO_ALL_ITEMS=5 ^
        cargo run > maxed.csv
    python plot_pareto_frontiers.py maxed.csv --output pareto_frontiers_maxed.png ^
        --title "Pareto frontiers (unlimited resources)"

CSV format produced by the solver:
    system,kind,floor,income,stability,defense
"""

import sys
import csv
import argparse
import matplotlib.pyplot as plt
from collections import defaultdict


def pareto_frontier(points):
    """Non-dominated set maximising both (x, income).

    Returns the frontier sorted ascending by x.
    """
    frontier = []
    for x, inc in points:
        dominated = any(
            ox >= x and oi >= inc and (ox > x or oi > inc) for ox, oi in points
        )
        if not dominated:
            frontier.append((x, inc))
    best = {}
    for x, inc in frontier:
        best[x] = max(best.get(x, inc), inc)
    return sorted(best.items())


def load_rows(source):
    if source == "-":
        reader = csv.DictReader(sys.stdin)
        return list(reader)
    with open(source, newline="", encoding="utf-8-sig") as f:
        return list(csv.DictReader(f))


def main():
    parser = argparse.ArgumentParser(
        description="Plot Pareto frontiers from SYSTEM_SOLVER_PARETO=1 CSV output."
    )
    parser.add_argument(
        "input", nargs="?", default="-",
        help="CSV file from solver (default: stdin; use '-' explicitly for stdin).",
    )
    parser.add_argument(
        "--output", default="pareto_frontiers.png",
        help="Output PNG filename (default: pareto_frontiers.png).",
    )
    parser.add_argument(
        "--title", default="Starsector colony Pareto frontiers",
        help="Main plot title.",
    )
    parser.add_argument(
        "--horizon", type=int, default=120,
        help="Horizon months shown in panel subtitles (default: 120).",
    )
    args = parser.parse_args()

    rows = load_rows(args.input)
    if not rows:
        print("No data rows found.", file=sys.stderr)
        sys.exit(1)

    # samples[system][kind] = [(income, stability, defense), ...]
    samples = defaultdict(list)
    for row in rows:
        samples[(row["system"], row["kind"])].append((
            float(row["income"]),
            float(row["stability"]),
            float(row["defense"]),
        ))

    systems = sorted({system for system, _ in samples.keys()})
    colors = {name: f"C{i}" for i, name in enumerate(systems)}

    fig, (ax_stab, ax_def) = plt.subplots(1, 2, figsize=(15, 6))
    fig.suptitle(args.title, fontsize=16, fontweight="bold")

    for system in systems:
        color = colors[system]

        stab_front = pareto_frontier(
            [(stab, inc) for inc, stab, _ in samples[(system, "stability")]]
        )
        if stab_front:
            xs, ys = zip(*stab_front)
            ax_stab.plot(xs, ys, marker="o", color=color, label=system)

        def_front = pareto_frontier(
            [(dfn, inc) for inc, _, dfn in samples[(system, "defense")]]
        )
        if def_front:
            xs, ys = zip(*def_front)
            ax_def.plot(xs, ys, marker="s", color=color, label=system)

    ax_stab.set_title(
        f"Pareto frontier: max income vs stability\n(horizon {args.horizon} months)"
    )
    ax_stab.set_xlabel("Average stability achieved")
    ax_stab.set_ylabel("Max net income (credits/month)")
    ax_stab.grid(True, alpha=0.3)
    ax_stab.legend()

    ax_def.set_title(
        f"Pareto frontier: max income vs ground defense\n(horizon {args.horizon} months)"
    )
    ax_def.set_xlabel("Average ground defense achieved")
    ax_def.set_ylabel("Max net income (credits/month)")
    ax_def.grid(True, alpha=0.3)
    ax_def.legend()

    fig.tight_layout()
    fig.savefig(args.output, dpi=120)
    print(f"wrote {args.output}")


if __name__ == "__main__":
    main()
