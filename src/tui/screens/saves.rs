use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::tui::app::Screen;
use crate::tui::jobs::Job;

use super::super::app::App;
use super::selected_style;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let rows: Vec<Row<'_>> = app
        .saves
        .iter()
        .map(|save| {
            let active = app
                .active_save
                .as_ref()
                .map(|row| row.dir_name == save.dir_name)
                .unwrap_or(false);
            Row::new(vec![
                Cell::from(save.character_name.clone()),
                Cell::from(save.dir_name.clone()),
                Cell::from(save.modified.clone()),
                Cell::from(save.extracted_at.clone().unwrap_or_else(|| "-".to_string())),
                Cell::from(if active { "(active)" } else { "" }),
            ])
        })
        .collect();
    let mut state = TableState::default().with_selected(Some(app.save_selection));
    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Percentage(35),
            Constraint::Length(14),
            Constraint::Length(14),
            Constraint::Length(10),
        ],
    )
    .header(Row::new([
        "character",
        "save dir",
        "modified",
        "extracted",
        "",
    ]))
    .block(
        Block::default()
            .title(format!("Saves · db {}", app.config.db_path.display()))
            .borders(Borders::ALL),
    )
    .row_highlight_style(selected_style())
    .highlight_symbol("> ");
    if app.saves.is_empty() {
        frame.render_widget(
            ratatui::widgets::Paragraph::new(
                app.error
                    .clone()
                    .unwrap_or_else(|| "loading saves...".to_string()),
            )
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().title("Saves").borders(Borders::ALL)),
            area,
        );
    } else {
        frame.render_stateful_widget(table, area, &mut state);
    }
}

pub fn handle_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('j') | KeyCode::Down => {
            app.save_selection = (app.save_selection + 1).min(app.saves.len().saturating_sub(1));
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.save_selection = app.save_selection.saturating_sub(1);
        }
        KeyCode::Enter => activate_or_extract(app),
        KeyCode::Char('e') => extract_selected(app),
        _ => {}
    }
}

fn activate_or_extract(app: &mut App) {
    let Some(save) = app.saves.get(app.save_selection).cloned() else {
        return;
    };
    if save.extracted_at.is_some() {
        if let Some(row) = find_extracted(app, &save.dir_name) {
            app.active_save = Some(row);
            app.active_screen = Screen::Rank;
            app.start_job(Job::LoadSystems {
                db_path: app.config.db_path.clone(),
                save: save.dir_name,
            });
        }
    } else {
        extract_selected(app);
    }
}

fn extract_selected(app: &mut App) {
    let Some(save) = app.saves.get(app.save_selection).cloned() else {
        return;
    };
    app.start_job(Job::Extract {
        db_path: app.config.db_path.clone(),
        starsector_dir: app.config.starsector_dir.clone(),
        save_dir: save.path,
    });
}

fn find_extracted(app: &App, dir_name: &str) -> Option<crate::extract::db::SaveRow> {
    app.active_save
        .as_ref()
        .filter(|row| row.dir_name == dir_name)
        .cloned()
        .or_else(|| {
            app.saves
                .iter()
                .find(|row| row.dir_name == dir_name && row.extracted_at.is_some())
                .map(|row| crate::extract::db::SaveRow {
                    id: 0,
                    dir_name: row.dir_name.clone(),
                    path: row.path.to_string_lossy().to_string(),
                    character_name: row.character_name.clone(),
                    save_date: row.modified.clone(),
                    game_version: String::new(),
                    character_level: 0,
                    extracted_at: row.extracted_at.clone().unwrap_or_default(),
                })
        })
}
