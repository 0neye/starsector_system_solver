use crate::solver::state::Action;
use crate::solver::{
    search_system_decomp_with_settings, search_system_maximize_with_settings, Balance, Goal,
    Metric, SolverSettings, State,
};
use crate::system::System;

#[derive(Debug, Clone)]
pub struct SolveOutcome {
    pub months: i32,
    pub achieved_income: f64,
    pub achieved_stability: f64,
    pub achieved_defense: f64,
    pub actions: Vec<Action>,
}

pub fn solve_goal(
    system: &System,
    balance: &Balance,
    goal: &Goal,
    time_limit: u32,
    include_industry_upgrades: bool,
) -> Option<SolveOutcome> {
    solve_goal_with_settings(
        system,
        balance,
        goal,
        time_limit,
        SolverSettings::legacy(include_industry_upgrades),
    )
}

pub fn solve_goal_with_settings(
    system: &System,
    balance: &Balance,
    goal: &Goal,
    time_limit: u32,
    settings: SolverSettings,
) -> Option<SolveOutcome> {
    let mut state = State::new(balance.clone(), system.clone());
    let replay_base = state.clone();
    let result = search_system_decomp_with_settings(&mut state, goal, time_limit, settings)
        .into_iter()
        .find(|result| result.solution.is_some())?;
    replay_outcome(replay_base, result.cost, result.solution.unwrap())
}

pub fn solve_maximize(
    system: &System,
    balance: &Balance,
    metric: Metric,
    floors: &Goal,
    horizon: i32,
    time_limit: u32,
    include_industry_upgrades: bool,
) -> Option<SolveOutcome> {
    solve_maximize_with_settings(
        system,
        balance,
        metric,
        floors,
        horizon,
        time_limit,
        SolverSettings::legacy(include_industry_upgrades),
    )
}

pub fn solve_maximize_with_settings(
    system: &System,
    balance: &Balance,
    metric: Metric,
    floors: &Goal,
    horizon: i32,
    time_limit: u32,
    settings: SolverSettings,
) -> Option<SolveOutcome> {
    let mut state = State::new(balance.clone(), system.clone());
    let replay_base = state.clone();
    let result = search_system_maximize_with_settings(
        &mut state, metric, floors, horizon, time_limit, settings,
    )
    .into_iter()
    .find(|result| result.solution.is_some())?;
    replay_outcome(replay_base, result.cost, result.solution.unwrap())
}

fn replay_outcome(mut replay: State, months: i32, actions: Vec<Action>) -> Option<SolveOutcome> {
    for action in &actions {
        replay.apply_action_raw(action, false);
    }
    Some(SolveOutcome {
        months,
        achieved_income: replay.balance().net_income(),
        achieved_stability: replay.system().avg_stability(),
        achieved_defense: replay.system().avg_ground_defense(),
        actions,
    })
}
