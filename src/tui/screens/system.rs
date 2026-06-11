use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::Frame;

use super::super::app::{App, Modal, Screen};
use super::selected_style;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);
    let selected_name = app
        .selected_system_name
        .clone()
        .or_else(|| app.visible_scope_names().first().cloned());
    let Some(selected_name) = selected_name else {
        frame.render_widget(
            Paragraph::new("No system loaded yet. Rank a save and press Enter on a row.")
                .block(Block::default().title("System").borders(Borders::ALL)),
            area,
        );
        return;
    };
    let Some(detail) = app.system_details.get(&selected_name) else {
        frame.render_widget(
            Paragraph::new("Planet detail for this system is not loaded.")
                .block(Block::default().title("System").borders(Borders::ALL)),
            area,
        );
        return;
    };
    let rows: Vec<Row<'_>> = detail
        .planets
        .iter()
        .map(|planet| {
            Row::new(vec![
                Cell::from(planet.row.name.clone()),
                Cell::from(planet.row.planet_type.clone()),
                Cell::from(format!("{:.0}%", planet.row.hazard_percent)),
                Cell::from(
                    planet
                        .row
                        .survey_level
                        .clone()
                        .unwrap_or_else(|| "-".to_string()),
                ),
                Cell::from(
                    planet
                        .row
                        .owner_faction
                        .clone()
                        .unwrap_or_else(|| "-".to_string()),
                ),
                Cell::from(planet.conditions.join(", ")),
            ])
        })
        .collect();
    let mut state = TableState::default().with_selected(Some(app.system_planet_selection));
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(18),
            Constraint::Length(14),
            Constraint::Length(8),
            Constraint::Length(12),
            Constraint::Length(14),
            Constraint::Percentage(40),
        ],
    )
    .header(Row::new([
        "planet",
        "type",
        "hazard",
        "survey",
        "owner",
        "conditions",
    ]))
    .block(
        Block::default()
            .title(format!(
                "System · {selected_name} · {} planets",
                detail.planets.len()
            ))
            .borders(Borders::ALL),
    )
    .row_highlight_style(selected_style())
    .highlight_symbol("> ");
    frame.render_stateful_widget(table, chunks[0], &mut state);

    let body = detail
        .planets
        .get(app.system_planet_selection)
        .map(|planet| {
            format!(
                "{} · {} · hazard {:.0}% · accessibility {}\nconditions: {}\nresources: farmland {:?}, ores {:?}, rare ores {:?}, volatiles {:?}, organics {:?}, ruins {:?}\ninfrastructure: {}",
                planet.row.name,
                planet.row.planet_type,
                planet.row.hazard_percent,
                planet
                    .row
                    .accessibility_percent
                    .map(|v| format!("{v:.0}%"))
                    .unwrap_or_else(|| "-".to_string()),
                planet.conditions.join(", "),
                planet.row.farmland,
                planet.row.ores,
                planet.row.rare_ores,
                planet.row.volatiles,
                planet.row.organics,
                planet.row.ruins,
                detail
                    .infrastructure
                    .iter()
                    .map(|row| row.infrastructure_type.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .unwrap_or_else(|| "No planets in system.".to_string());
    frame.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: true }).block(
            Block::default()
                .title("Planet detail")
                .borders(Borders::ALL),
        ),
        chunks[1],
    );
}

pub fn handle_key(app: &mut App, code: KeyCode) {
    if app.editing_filter {
        match code {
            KeyCode::Esc | KeyCode::Enter => app.editing_filter = false,
            KeyCode::Backspace => {
                app.rank_filter.pop();
            }
            KeyCode::Char(c) => {
                app.rank_filter.push(c);
                let needle = app.rank_filter.to_lowercase();
                if let Some(name) = app
                    .visible_scope_names()
                    .into_iter()
                    .find(|name| name.to_lowercase().contains(&needle))
                {
                    app.selected_system_name = Some(name);
                    app.system_planet_selection = 0;
                }
            }
            _ => {}
        }
        return;
    }
    let max = app
        .selected_system_name
        .as_ref()
        .and_then(|name| app.system_details.get(name))
        .map(|detail| detail.planets.len().saturating_sub(1))
        .unwrap_or(0);
    match code {
        KeyCode::Char('j') | KeyCode::Down => {
            app.system_planet_selection = (app.system_planet_selection + 1).min(max)
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.system_planet_selection = app.system_planet_selection.saturating_sub(1)
        }
        KeyCode::Char('b') => app.active_screen = Screen::Rank,
        KeyCode::Char('s') => app.open_solve_for_selected_system(),
        KeyCode::Char('S') => app.modal = Some(Modal::Settings),
        KeyCode::Char('/') => {
            app.editing_filter = true;
            app.rank_filter.clear();
            app.status = "type to search systems in active scope".to_string();
        }
        _ => {}
    }
}
