use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Rect};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::constants::ColonyItem;
use crate::tui::config::DiscoveryDefinition;

use super::super::app::{App, Modal};
use super::selected_style;

const BASE_FIELDS: usize = 12;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let rows = setting_rows(app);
    let mut state = TableState::default().with_selected(Some(app.settings_selection));
    let table = Table::new(rows, [Constraint::Length(28), Constraint::Min(20)])
        .block(
            Block::default()
                .title(settings_title(app))
                .borders(Borders::ALL),
        )
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
            if let Err(err) = app.config.save(app.config_path().to_path_buf()) {
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
        Row::new(vec![
            Cell::from("credits"),
            Cell::from(setting_value(app, 0, format!("{:.0}", app.config.credits))),
        ]),
        Row::new(vec![
            Cell::from("story points"),
            Cell::from(setting_value(app, 1, app.config.story_points.to_string())),
        ]),
        Row::new(vec![
            Cell::from("alpha cores"),
            Cell::from(setting_value(app, 2, app.config.alpha_cores.to_string())),
        ]),
        Row::new(vec![
            Cell::from("horizon months"),
            Cell::from(setting_value(app, 3, app.config.horizon_months.to_string())),
        ]),
        Row::new(vec![
            Cell::from("time budget ms"),
            Cell::from(setting_value(
                app,
                4,
                app.config.solver_time_budget_ms.to_string(),
            )),
        ]),
        Row::new(vec![
            Cell::from("discovery definition"),
            Cell::from(format!("{:?}", app.config.discovery_definition)),
        ]),
        Row::new(vec![
            Cell::from("include core worlds"),
            Cell::from(app.config.include_core_worlds.to_string()),
        ]),
        Row::new(vec![
            Cell::from("rank by score/planet"),
            Cell::from(app.config.rank_by_score_per_planet.to_string()),
        ]),
        Row::new(vec![
            Cell::from("industry upgrades"),
            Cell::from(app.config.include_industry_upgrades.to_string()),
        ]),
        Row::new(vec![
            Cell::from("parallel builds"),
            Cell::from(app.config.allow_parallel_builds.to_string()),
        ]),
        Row::new(vec![
            Cell::from("db path"),
            Cell::from(setting_value(
                app,
                10,
                app.config.db_path.display().to_string(),
            )),
        ]),
        Row::new(vec![
            Cell::from("starsector_dir override"),
            Cell::from(setting_value(
                app,
                11,
                app.config
                    .starsector_dir
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default(),
            )),
        ]),
    ];
    for (offset, item) in ColonyItem::all().into_iter().enumerate() {
        let count = app
            .config
            .colony_items
            .get(item.name())
            .copied()
            .unwrap_or(0);
        rows.push(Row::new(vec![
            Cell::from(format!("item: {}", item.name())),
            Cell::from(setting_value(app, BASE_FIELDS + offset, count.to_string())),
        ]));
    }
    rows.push(Row::new(vec![
        Cell::from("reset to defaults"),
        Cell::from("Enter"),
    ]));
    rows
}

fn settings_title(app: &App) -> &'static str {
    if app.settings_editing {
        "Setup - editing (Enter commit, Esc cancel)"
    } else {
        "Setup"
    }
}

fn setting_value(app: &App, index: usize, value: String) -> String {
    if app.settings_editing && app.settings_selection == index {
        format!("editing: {}_", app.settings_input)
    } else {
        value
    }
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
        7 => {
            app.config.rank_by_score_per_planet = !app.config.rank_by_score_per_planet;
            app.status = format!(
                "rank sort: {}",
                if app.config.rank_by_score_per_planet {
                    "score/planet"
                } else {
                    "score"
                }
            );
        }
        8 => {
            app.config.include_industry_upgrades = !app.config.include_industry_upgrades;
            app.mark_rank_stale();
            app.status = format!(
                "industry upgrades: {}",
                if app.config.include_industry_upgrades {
                    "enabled"
                } else {
                    "disabled"
                }
            );
        }
        9 => {
            app.config.allow_parallel_builds = !app.config.allow_parallel_builds;
            app.mark_rank_stale();
            app.status = format!(
                "parallel builds: {}",
                if app.config.allow_parallel_builds {
                    "enabled"
                } else {
                    "disabled"
                }
            );
        }
        10 => edit(app, app.config.db_path.display().to_string()),
        11 => edit(
            app,
            app.config
                .starsector_dir
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        ),
        n if n == BASE_FIELDS + ColonyItem::all().len() => {
            // Reset the solver-facing settings but keep machine-specific
            // paths: wiping db_path/starsector_dir would strand the user.
            let db_path = std::mem::take(&mut app.config.db_path);
            let starsector_dir = app.config.starsector_dir.take();
            app.config = Default::default();
            app.config.db_path = db_path;
            app.config.starsector_dir = starsector_dir;
            app.mark_rank_stale();
            app.status = "settings reset to defaults (paths kept)".to_string();
        }
        n if n >= BASE_FIELDS && n < BASE_FIELDS + ColonyItem::all().len() => {
            let item = ColonyItem::all()[n - BASE_FIELDS];
            let count = app
                .config
                .colony_items
                .get(item.name())
                .copied()
                .unwrap_or(0);
            edit(app, count.to_string());
        }
        _ => {}
    }
}

fn edit(app: &mut App, value: String) {
    app.settings_input = value;
    app.settings_editing = true;
}

fn commit_edit(app: &mut App) {
    let input = app.settings_input.trim().to_string();
    let mut balance_changed = false;
    let mut parse_failed = false;
    // Numeric fields: a failed parse keeps the editor open with feedback
    // instead of silently discarding the input.
    macro_rules! numeric {
        ($target:expr) => {
            match input.parse() {
                Ok(v) => {
                    $target = v;
                    balance_changed = true;
                }
                Err(_) => parse_failed = true,
            }
        };
    }
    match app.settings_selection {
        0 => numeric!(app.config.credits),
        1 => numeric!(app.config.story_points),
        2 => numeric!(app.config.alpha_cores),
        3 => numeric!(app.config.horizon_months),
        4 => numeric!(app.config.solver_time_budget_ms),
        10 => app.config.db_path = input.as_str().into(),
        11 => {
            app.config.starsector_dir = if input.is_empty() {
                None
            } else {
                Some(input.as_str().into())
            };
        }
        n if n >= BASE_FIELDS && n < BASE_FIELDS + ColonyItem::all().len() => {
            let item = ColonyItem::all()[n - BASE_FIELDS];
            match input.parse::<u32>() {
                Ok(count) => {
                    if count == 0 {
                        app.config.colony_items.remove(item.name());
                    } else {
                        app.config
                            .colony_items
                            .insert(item.name().to_string(), count);
                    }
                    balance_changed = true;
                }
                Err(_) => parse_failed = true,
            }
        }
        _ => {}
    }
    if parse_failed {
        app.status = format!("invalid number: {input}");
        return;
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
    let entry = app
        .config
        .colony_items
        .entry(item.name().to_string())
        .or_default();
    if delta < 0 {
        *entry = entry.saturating_sub(1);
    } else {
        *entry += 1;
    }
    app.mark_rank_stale();
}
