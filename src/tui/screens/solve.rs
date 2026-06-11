use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;

use crate::solver::pareto::ParetoSolve;
use crate::solver::Metric;
use crate::tui::app::{pareto_points, App, SolveFocus, SolveMode, SolveResult};

use super::selected_style;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    if app.selected_system_name.is_none() {
        frame.render_widget(
            Paragraph::new("Open a system first, then press 4 or s from System detail.")
                .block(Block::default().title("Solve").borders(Borders::ALL)),
            area,
        );
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
        .split(area);
    draw_params(frame, app, chunks[0]);
    draw_results(frame, app, chunks[1]);
}

fn draw_params(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let rows = param_rows(app);
    let selected = if app.solve_focus == SolveFocus::Parameters {
        Some(app.solve_param_selection.min(rows.len().saturating_sub(1)))
    } else {
        None
    };
    let mut state = TableState::default().with_selected(selected);
    let title = format!(
        "Parameters · {} · balance via Setup",
        app.selected_system_name.as_deref().unwrap_or("-")
    );
    let table = Table::new(rows, [Constraint::Length(18), Constraint::Min(12)])
        .block(Block::default().title(title).borders(Borders::ALL))
        .row_highlight_style(selected_style())
        .highlight_symbol("> ");
    frame.render_stateful_widget(table, area, &mut state);
}

fn draw_results(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let Some(result) = app.solve_result.as_ref() else {
        frame.render_widget(
            Paragraph::new(
                "Press R to run. The time budget is enforced; x cancels a running solve.",
            )
            .wrap(Wrap { trim: true })
            .block(Block::default().title("Results").borders(Borders::ALL)),
            area,
        );
        return;
    };
    match result {
        SolveResult::Pareto(solve) => draw_pareto(frame, app, area, solve),
        SolveResult::Goal(outcome) | SolveResult::Maximize(outcome) => {
            let text = if let Some(outcome) = outcome {
                format!(
                    "months: {}\nincome: {:.0}\nstability: {:.1}\ndefense: {:.1}\n\nactions: {}\n\nEnter or p opens Plan.",
                    outcome.months,
                    outcome.achieved_income,
                    outcome.achieved_stability,
                    outcome.achieved_defense,
                    outcome.actions.len()
                )
            } else {
                "No solution found within the time limit.".to_string()
            };
            frame.render_widget(
                Paragraph::new(text)
                    .wrap(Wrap { trim: true })
                    .block(Block::default().title("Results").borders(Borders::ALL)),
                area,
            );
        }
    }
}

fn draw_pareto(frame: &mut Frame<'_>, app: &App, area: Rect, solve: &ParetoSolve) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(4)])
        .split(area);
    frame.render_widget(
        Paragraph::new(format!(
            "score {:.1} · stability AUC {:.0} · defense AUC {:.0}",
            solve.score, solve.stability_auc, solve.defense_auc
        ))
        .block(Block::default().title("Summary").borders(Borders::ALL)),
        chunks[0],
    );
    let recommendation = solve.recommendation.as_ref();
    let points = pareto_points(solve);
    let rows: Vec<Row<'_>> = points
        .iter()
        .map(|point| {
            let mark = if recommendation
                .map(|rec| {
                    rec.kind == point.kind
                        && (rec.floor - point.floor).abs() < f64::EPSILON
                        && rec.months == point.months
                })
                .unwrap_or(false)
            {
                "*"
            } else {
                ""
            };
            Row::new(vec![
                Cell::from(mark),
                Cell::from(point.kind.as_str()),
                Cell::from(format!("{:.0}", point.floor)),
                Cell::from(format!("{:.0}", point.income)),
                Cell::from(format!("{:.1}", point.stability)),
                Cell::from(format!("{:.1}", point.defense)),
                Cell::from(point.months.to_string()),
            ])
        })
        .collect();
    let selected = if app.solve_focus == SolveFocus::Results {
        Some(
            app.solve_result_selection
                .min(points.len().saturating_sub(1)),
        )
    } else {
        None
    };
    let mut state = TableState::default().with_selected(selected);
    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Length(7),
        ],
    )
    .header(Row::new([
        "",
        "frontier",
        "floor",
        "income",
        "stability",
        "defense",
        "month",
    ]))
    .block(Block::default().title("Frontiers").borders(Borders::ALL))
    .row_highlight_style(selected_style())
    .highlight_symbol("> ");
    frame.render_stateful_widget(table, chunks[1], &mut state);
}

pub fn handle_key(app: &mut App, code: KeyCode) {
    if app.editing_solve_param {
        match code {
            KeyCode::Esc => app.editing_solve_param = false,
            KeyCode::Enter => commit_edit(app),
            KeyCode::Backspace => {
                app.solve_input.pop();
            }
            KeyCode::Char(c) => app.solve_input.push(c),
            _ => {}
        }
        return;
    }

    match code {
        KeyCode::Tab => {
            app.solve_focus = match app.solve_focus {
                SolveFocus::Parameters => SolveFocus::Results,
                SolveFocus::Results => SolveFocus::Parameters,
            }
        }
        KeyCode::Char('b') => app.active_screen = crate::tui::app::Screen::System,
        KeyCode::Char('R') | KeyCode::Char('r') => app.start_solve(),
        KeyCode::Char('m') => {
            cycle_mode(app, 1);
            app.restore_solve_cache();
        }
        KeyCode::Enter if app.solve_focus == SolveFocus::Results => app.open_plan(),
        KeyCode::Char('p') => app.open_plan(),
        KeyCode::Enter => activate_param(app),
        KeyCode::Char('j') | KeyCode::Down => move_selection(app, 1),
        KeyCode::Char('k') | KeyCode::Up => move_selection(app, -1),
        KeyCode::Left => {
            if app.solve_focus == SolveFocus::Parameters && app.solve_param_selection == 0 {
                cycle_mode(app, -1);
                app.restore_solve_cache();
            } else if app.solve_focus == SolveFocus::Parameters
                && app.solve_params.mode == SolveMode::Maximize
                && app.solve_param_selection == 1
            {
                cycle_metric(app, -1);
                app.restore_solve_cache();
            }
        }
        KeyCode::Right => {
            if app.solve_focus == SolveFocus::Parameters && app.solve_param_selection == 0 {
                cycle_mode(app, 1);
                app.restore_solve_cache();
            } else if app.solve_focus == SolveFocus::Parameters
                && app.solve_params.mode == SolveMode::Maximize
                && app.solve_param_selection == 1
            {
                cycle_metric(app, 1);
                app.restore_solve_cache();
            }
        }
        _ => {}
    }
}

fn move_selection(app: &mut App, delta: i32) {
    if app.solve_focus == SolveFocus::Results {
        let max = match app.solve_result.as_ref() {
            Some(SolveResult::Pareto(solve)) => pareto_points(solve).len().saturating_sub(1),
            _ => 0,
        };
        app.solve_result_selection = if delta < 0 {
            app.solve_result_selection.saturating_sub(1)
        } else {
            (app.solve_result_selection + 1).min(max)
        };
    } else {
        let max = param_rows(app).len().saturating_sub(1);
        app.solve_param_selection = if delta < 0 {
            app.solve_param_selection.saturating_sub(1)
        } else {
            (app.solve_param_selection + 1).min(max)
        };
    }
}

fn activate_param(app: &mut App) {
    let last = param_rows(app).len().saturating_sub(1);
    if app.solve_param_selection == 0 {
        cycle_mode(app, 1);
        app.restore_solve_cache();
    } else if app.solve_params.mode == SolveMode::Maximize && app.solve_param_selection == 1 {
        cycle_metric(app, 1);
        app.restore_solve_cache();
    } else if app.solve_param_selection == last {
        app.start_solve();
    } else {
        app.solve_input = current_param_value(app);
        app.editing_solve_param = true;
    }
}

fn cycle_metric(app: &mut App, delta: i32) {
    let metrics = [Metric::Income, Metric::Stability, Metric::Defense];
    let current = metrics
        .iter()
        .position(|metric| *metric == app.solve_params.maximize_metric)
        .unwrap_or(0);
    let next = if delta < 0 {
        current.saturating_sub(1)
    } else {
        (current + 1) % metrics.len()
    };
    app.solve_params.maximize_metric = metrics[next];
}

fn cycle_mode(app: &mut App, delta: i32) {
    let modes = [SolveMode::Pareto, SolveMode::Goal, SolveMode::Maximize];
    let current = modes
        .iter()
        .position(|mode| *mode == app.solve_params.mode)
        .unwrap_or(0);
    let next = if delta < 0 {
        current.saturating_sub(1)
    } else {
        (current + 1) % modes.len()
    };
    app.solve_params.mode = modes[next];
    app.solve_param_selection = app
        .solve_param_selection
        .min(param_rows(app).len().saturating_sub(1));
}

fn commit_edit(app: &mut App) {
    let value = app.solve_input.trim();
    let ok = match (app.solve_params.mode, app.solve_param_selection) {
        (SolveMode::Goal, 1) => parse_f64(value, &mut app.solve_params.goal_income),
        (SolveMode::Goal, 2) => parse_i32(value, &mut app.solve_params.goal_stability),
        (SolveMode::Goal, 3) => parse_f64(value, &mut app.solve_params.goal_defense),
        (SolveMode::Goal, 4) | (SolveMode::Pareto, 1) | (SolveMode::Maximize, 4) => {
            parse_i32(value, &mut app.solve_params.horizon)
        }
        (SolveMode::Goal, 5) | (SolveMode::Pareto, 2) | (SolveMode::Maximize, 5) => {
            parse_u32(value, &mut app.solve_params.time_limit)
        }
        (SolveMode::Maximize, selection) => match max_floor_field(app, selection) {
            Some(MaxFloorField::Income) => parse_f64(value, &mut app.solve_params.floor_income),
            Some(MaxFloorField::Stability) => {
                parse_i32(value, &mut app.solve_params.floor_stability)
            }
            Some(MaxFloorField::Defense) => parse_f64(value, &mut app.solve_params.floor_defense),
            None => true,
        },
        _ => true,
    };
    if !ok {
        // Keep editing so the user can fix the value; Esc still cancels.
        app.status = format!("invalid number: {value}");
        return;
    }
    app.editing_solve_param = false;
    app.restore_solve_cache();
}

fn parse_f64(value: &str, target: &mut f64) -> bool {
    match value.parse() {
        Ok(parsed) => {
            *target = parsed;
            true
        }
        Err(_) => false,
    }
}

fn parse_i32(value: &str, target: &mut i32) -> bool {
    match value.parse() {
        Ok(parsed) => {
            *target = parsed;
            true
        }
        Err(_) => false,
    }
}

fn parse_u32(value: &str, target: &mut u32) -> bool {
    match value.parse() {
        Ok(parsed) => {
            *target = parsed;
            true
        }
        Err(_) => false,
    }
}

fn current_param_value(app: &App) -> String {
    match (app.solve_params.mode, app.solve_param_selection) {
        (SolveMode::Goal, 1) => app.solve_params.goal_income.to_string(),
        (SolveMode::Goal, 2) => app.solve_params.goal_stability.to_string(),
        (SolveMode::Goal, 3) => app.solve_params.goal_defense.to_string(),
        (SolveMode::Goal, 4) | (SolveMode::Pareto, 1) | (SolveMode::Maximize, 4) => {
            app.solve_params.horizon.to_string()
        }
        (SolveMode::Goal, 5) | (SolveMode::Pareto, 2) | (SolveMode::Maximize, 5) => {
            app.solve_params.time_limit.to_string()
        }
        (SolveMode::Maximize, selection) => match max_floor_field(app, selection) {
            Some(MaxFloorField::Income) => app.solve_params.floor_income.to_string(),
            Some(MaxFloorField::Stability) => app.solve_params.floor_stability.to_string(),
            Some(MaxFloorField::Defense) => app.solve_params.floor_defense.to_string(),
            None => String::new(),
        },
        _ => String::new(),
    }
}

fn param_rows(app: &App) -> Vec<Row<'_>> {
    let p = &app.solve_params;
    let mut rows = vec![Row::new(vec![
        Cell::from("mode"),
        Cell::from(p.mode.as_str()),
    ])];
    let editing = |index: usize, value: String| -> String {
        if app.editing_solve_param && app.solve_param_selection == index {
            format!("editing: {}_", app.solve_input)
        } else {
            value
        }
    };
    match p.mode {
        SolveMode::Pareto => {}
        SolveMode::Goal => {
            rows.push(Row::new(vec![
                Cell::from("income"),
                Cell::from(editing(1, format!("{:.0}", p.goal_income))),
            ]));
            rows.push(Row::new(vec![
                Cell::from("stability"),
                Cell::from(editing(2, p.goal_stability.to_string())),
            ]));
            rows.push(Row::new(vec![
                Cell::from("defense"),
                Cell::from(editing(3, format!("{:.0}", p.goal_defense))),
            ]));
        }
        SolveMode::Maximize => {
            rows.push(Row::new(vec![
                Cell::from("metric"),
                Cell::from(p.maximize_metric.as_str()),
            ]));
            if p.maximize_metric != Metric::Income {
                rows.push(Row::new(vec![
                    Cell::from("income floor"),
                    Cell::from(editing(rows.len(), format!("{:.0}", p.floor_income))),
                ]));
            }
            if p.maximize_metric != Metric::Stability {
                rows.push(Row::new(vec![
                    Cell::from("stability floor"),
                    Cell::from(editing(rows.len(), p.floor_stability.to_string())),
                ]));
            }
            if p.maximize_metric != Metric::Defense {
                rows.push(Row::new(vec![
                    Cell::from("defense floor"),
                    Cell::from(editing(rows.len(), format!("{:.0}", p.floor_defense))),
                ]));
            }
        }
    }
    rows.push(Row::new(vec![
        Cell::from("horizon"),
        Cell::from(editing(rows.len(), p.horizon.to_string())),
    ]));
    rows.push(Row::new(vec![
        Cell::from("time budget ms"),
        Cell::from(editing(rows.len(), p.time_limit.to_string())),
    ]));
    rows.push(Row::new(vec![
        Cell::from("[Run]").style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Green),
        ),
        Cell::from("Enter or R"),
    ]));
    rows
}

#[derive(Clone, Copy)]
enum MaxFloorField {
    Income,
    Stability,
    Defense,
}

fn max_floor_field(app: &App, selection: usize) -> Option<MaxFloorField> {
    let fields: Vec<MaxFloorField> = [
        (Metric::Income, MaxFloorField::Income),
        (Metric::Stability, MaxFloorField::Stability),
        (Metric::Defense, MaxFloorField::Defense),
    ]
    .into_iter()
    .filter_map(|(metric, field)| (metric != app.solve_params.maximize_metric).then_some(field))
    .collect();
    selection
        .checked_sub(2)
        .and_then(|index| fields.get(index).copied())
}
