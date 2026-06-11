use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Rect};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::constants::ColonyItem;
use crate::tui::config::{DiscoveryDefinition, CONFIG_PATH};

use super::super::app::{App, Modal};
use super::selected_style;

const BASE_FIELDS: usize = 9;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let rows = setting_rows(app);
    let mut state = TableState::default().with_selected(Some(app.settings_selection));
    let table = Table::new(rows, [Constraint::Length(28), Constraint::Min(20)])
        .block(Block::default().title("Setup").borders(Borders::ALL))
        .row_highlight_style(selected_style())
        .highlight_symbol("> ");
    frame.render_stateful_widget(table, area, &mut state);
}

pub fn handle_key(app: &mut App, code: KeyCode) {
    let count = BASE_FIELDS + ColonyItem::all().len() + 1;
    if app.settings_editing {
        match code {
            KeyCode::Esc => app.settings_editing = false,
            KeyCode::Enter => commit_edit(app),
            KeyCode::Backspace => {
                app.settings_input.pop();
            }
            KeyCode::Char(c) => app.settings_input.push(c),
            _ => {}
        }
        return;
    }
    match code {
        KeyCode::Esc => {
            if let Err(err) = app.config.save(CONFIG_PATH) {
                app.status = err;
            } else {
                app.status = "settings saved".to_string();
            }
            app.modal = None;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.settings_selection = (app.settings_selection + 1).min(count.saturating_sub(1))
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.settings_selection = app.settings_selection.saturating_sub(1)
        }
        KeyCode::Enter => begin_edit_or_toggle(app),
        KeyCode::Char('+') => adjust_item(app, 1),
        KeyCode::Char('-') => adjust_item(app, -1),
        _ => {}
    }
}

fn setting_rows(app: &App) -> Vec<Row<'_>> {
    let mut rows = vec![
        Row::new(vec![Cell::from("credits"), Cell::from(format!("{:.0}", app.config.credits))]),
        Row::new(vec![Cell::from("story points"), Cell::from(app.config.story_points.to_string())]),
        Row::new(vec![Cell::from("alpha cores"), Cell::from(app.config.alpha_cores.to_string())]),
        Row::new(vec![Cell::from("horizon months"), Cell::from(app.config.horizon_months.to_string())]),
        Row::new(vec![Cell::from("time budget ms"), Cell::from(app.config.solver_time_budget_ms.to_string())]),
        Row::new(vec![Cell::from("discovery definition"), Cell::from(format!("{:?}", app.config.discovery_definition))]),
        Row::new(vec![Cell::from("include core worlds"), Cell::from(app.config.include_core_worlds.to_string())]),
        Row::new(vec![Cell::from("db path"), Cell::from(app.config.db_path.display().to_string())]),
        Row::new(vec![
            Cell::from("starsector_dir override"),
            Cell::from(
                app.config
                    .starsector_dir
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
            ),
        ]),
    ];
    for item in ColonyItem::all() {
        let count = app.config.colony_items.get(item.name()).copied().unwrap_or(0);
        rows.push(Row::new(vec![
            Cell::from(format!("item: {}", item.name())),
            Cell::from(count.to_string()),
        ]));
    }
    rows.push(Row::new(vec![Cell::from("reset to defaults"), Cell::from("Enter")]));
    rows
}

fn begin_edit_or_toggle(app: &mut App) {
    match app.settings_selection {
        0 => edit(app, app.config.credits.to_string()),
        1 => edit(app, app.config.story_points.to_string()),
        2 => edit(app, app.config.alpha_cores.to_string()),
        3 => edit(app, app.config.horizon_months.to_string()),
        4 => edit(app, app.config.solver_time_budget_ms.to_string()),
        5 => {
            app.config.discovery_definition = match app.config.discovery_definition {
                DiscoveryDefinition::AtLeastOneSurveyed => DiscoveryDefinition::FullySurveyed,
                DiscoveryDefinition::FullySurveyed => DiscoveryDefinition::AtLeastOneSurveyed,
            };
        }
        6 => app.config.include_core_worlds = !app.config.include_core_worlds,
        7 => edit(app, app.config.db_path.display().to_string()),
        8 => edit(
            app,
            app.config
                .starsector_dir
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        ),
        n if n == BASE_FIELDS + ColonyItem::all().len() => {
            app.config = Default::default();
            app.mark_rank_stale();
        }
        _ => {}
    }
}

fn edit(app: &mut App, value: String) {
    app.settings_input = value;
    app.settings_editing = true;
}

fn commit_edit(app: &mut App) {
    let input = app.settings_input.trim();
    let mut balance_changed = false;
    match app.settings_selection {
        0 => {
            if let Ok(v) = input.parse() {
                app.config.credits = v;
                balance_changed = true;
            }
        }
        1 => {
            if let Ok(v) = input.parse() {
                app.config.story_points = v;
                balance_changed = true;
            }
        }
        2 => {
            if let Ok(v) = input.parse() {
                app.config.alpha_cores = v;
                balance_changed = true;
            }
        }
        3 => {
            if let Ok(v) = input.parse() {
                app.config.horizon_months = v;
                balance_changed = true;
            }
        }
        4 => {
            if let Ok(v) = input.parse() {
                app.config.solver_time_budget_ms = v;
                balance_changed = true;
            }
        }
        7 => app.config.db_path = input.into(),
        8 => {
            app.config.starsector_dir = if input.is_empty() {
                None
            } else {
                Some(input.into())
            };
        }
        _ => {}
    }
    app.settings_editing = false;
    if balance_changed {
        app.mark_rank_stale();
    }
}

fn adjust_item(app: &mut App, delta: i32) {
    if app.settings_selection < BASE_FIELDS
        || app.settings_selection >= BASE_FIELDS + ColonyItem::all().len()
    {
        return;
    }
    let item = ColonyItem::all()[app.settings_selection - BASE_FIELDS];
    let entry = app.config.colony_items.entry(item.name().to_string()).or_default();
    if delta < 0 {
        *entry = entry.saturating_sub(1);
    } else {
        *entry += 1;
    }
    app.mark_rank_stale();
}
