use std::collections::HashMap;
use std::time::Instant;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::solver::pareto::ParetoSolve;
use crate::solver::{
    solve_pareto_bound_with_settings, solve_pareto_quick_with_settings,
    solve_pareto_template_with_settings, Balance, SolverSettings,
};
use crate::system::System;

/// `--rank` scoring strategy. `Quick` is the Tier-1 budgeted search; `Template`
/// and `Bound` are the two Tier-0 instant scorers (in practice a lower and an
/// upper bound on the score, respectively -- see the soundness caveats on
/// `solve_pareto_template` / `solve_pareto_bound`). See workspace/QUICK_RANKING_DESIGN.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
pub enum RankScorer {
    Quick,
    Template,
    Bound,
}

#[derive(Debug, Clone)]
pub struct RankRow {
    pub system: String,
    pub planet_count: usize,
    pub solve: ParetoSolve,
    pub seconds: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum RankSortMode {
    ScorePerPlanet,
    TotalScore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoRankSystemMatches;

/// Peak income across both frontiers, matching the CLI's ranking export metric.
pub fn peak_income(solve: &ParetoSolve) -> f64 {
    solve
        .stability_frontier
        .iter()
        .chain(solve.defense_frontier.iter())
        .map(|p| p.income)
        .fold(0.0_f64, f64::max)
}

/// Apply the `--rank-system` filter without tying library callers to process
/// exits or terminal output.
pub fn filter_system_names<'a>(
    systems: &'a HashMap<String, System>,
    filters: &[String],
) -> Result<Vec<&'a String>, NoRankSystemMatches> {
    let mut names: Vec<&String> = systems.keys().collect();
    names.sort();

    if filters.is_empty() {
        return Ok(names);
    }

    let needles: Vec<String> = filters.iter().map(|f| f.to_lowercase()).collect();
    names.retain(|name| {
        let lower = name.to_lowercase();
        needles.iter().any(|needle| lower.contains(needle))
    });

    if names.is_empty() {
        Err(NoRankSystemMatches)
    } else {
        Ok(names)
    }
}

/// Best-first ordering used by both CLI and TUI rank presenters.
pub fn sort_rows_best_first(rows: &mut [RankRow], mode: RankSortMode) {
    rows.sort_by(|a, b| {
        rank_value(b, mode)
            .total_cmp(&rank_value(a, mode))
            .then_with(|| a.system.cmp(&b.system))
    });
}

pub fn score_per_planet(row: &RankRow) -> f64 {
    if row.planet_count == 0 {
        0.0
    } else {
        row.solve.score / row.planet_count as f64
    }
}

fn rank_value(row: &RankRow, mode: RankSortMode) -> f64 {
    match mode {
        RankSortMode::ScorePerPlanet => score_per_planet(row),
        RankSortMode::TotalScore => row.solve.score,
    }
}

/// Score selected systems in display order while exposing each finished row to
/// interactive presenters before the final best-first sort.
#[allow(clippy::too_many_arguments)]
pub fn rank_systems(
    systems: &HashMap<String, System>,
    balance: &Balance,
    names: &[&String],
    horizon: i32,
    time_limit: u32,
    scorer: RankScorer,
    settings: SolverSettings,
    on_scored: &mut dyn FnMut(&RankRow),
) -> Vec<RankRow> {
    let scorer_fn: fn(&System, &Balance, i32, u32, SolverSettings) -> ParetoSolve = match scorer {
        RankScorer::Quick => solve_pareto_quick_with_settings,
        RankScorer::Template => solve_pareto_template_with_settings,
        RankScorer::Bound => solve_pareto_bound_with_settings,
    };

    let mut rows = Vec::with_capacity(names.len());
    for name in names {
        // Stop scoring further systems on cooperative cancel; rows scored so
        // far are still sorted and returned.
        if crate::solver::cancel::is_cancelled() {
            break;
        }
        let t0 = Instant::now();
        let solve = scorer_fn(&systems[*name], balance, horizon, time_limit, settings);
        let row = RankRow {
            system: (*name).clone(),
            planet_count: systems[*name].planets().len(),
            solve,
            seconds: t0.elapsed().as_secs_f64(),
        };
        on_scored(&row);
        rows.push(row);
    }

    sort_rows_best_first(&mut rows, RankSortMode::ScorePerPlanet);
    rows
}
