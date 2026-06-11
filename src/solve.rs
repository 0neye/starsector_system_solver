use crate::solver::{
    search_system_decomp, search_system_maximize, Balance, Goal, Metric, State,
};
use crate::solver::state::Action;
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
) -> Option<SolveOutcome> {
    let mut state = State::new(balance.clone(), system.clone());
    let replay_base = state.clone();
    let result = search_system_decomp(&mut state, goal, time_limit, true)
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
) -> Option<SolveOutcome> {
    let mut state = State::new(balance.clone(), system.clone());
    let replay_base = state.clone();
    let result = search_system_maximize(&mut state, metric, floors, horizon, time_limit, true)
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
