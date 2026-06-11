use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use crate::rank::{peak_income, RankScorer};

use super::super::app::{format_duration, estimate_rank_cost, App, Modal, ScopeMode};
use super::selected_style;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let visible = app.visible_rank_rows();
    let total = app.systems.len();
    let scope_count = app.visible_scope_names().len();
    let header = format!(
        "Rank · scope: {} ({} of {}) · scorer: {:?} · {}",
        match app.scope_mode {
            ScopeMode::Discovered => "discovered",
            ScopeMode::All => "all",
        },
        scope_count,
        total,
        app.scorer,
        if visible.is_empty() {
            format!(
                "press r to rank ~{} systems (~{})",
                scope_count,
                format_duration(estimate_rank_cost(scope_count, app.scorer))
            )
        } else {
            format!("{} rows", visible.len())
        }
    );
    let mut rows = Vec::new();
    for (idx, row) in visible.iter().enumerate() {
        rows.push(Row::new(vec![
            Cell::from((idx + 1).to_string()),
            Cell::from(row.system.clone()),
            Cell::from(format!("{:.1}", row.solve.score)),
            Cell::from(format!("{:.0}", peak_income(&row.solve))),
            Cell::from(format!("{:.1}s", row.seconds)),
        ]));
    }
    let mut state = TableState::default().with_selected(Some(
        app.rank_selection.min(visible.len().saturating_sub(1)),
    ));
    let table = Table::new(
        rows,
        [
            Constraint::Length(5),
            Constraint::Percentage(45),
            Constraint::Length(10),
            Constraint::Length(14),
            Constraint::Length(9),
        ],
    )
    .header(Row::new(["#", "system", "score", "peak income", "time"]))
    .block(Block::default().title(header).borders(Borders::ALL))
    .row_highlight_style(selected_style())
    .highlight_symbol("> ");
    frame.render_stateful_widget(table, area, &mut state);
    if !app.rank_filter.is_empty() || app.editing_filter {
        let filter = format!("/{}", app.rank_filter);
        let block = Paragraph::new(filter)
            .style(Style::default().fg(Color::Cyan))
            .block(Block::default().borders(Borders::ALL).title("Filter"));
        let popup = Rect::new(area.x + 2, area.y + area.height.saturating_sub(3), 40, 3);
        frame.render_widget(block, popup);
    }
}

pub fn draw_scorer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let text = [
        ("quick", "slow-ish real search; best ordering signal"),
        ("template", "instant rough lower-bound portfolio"),
        ("bound", "about 1s/system credit-relaxed ceiling"),
    ]
    .into_iter()
    .map(|(name, desc)| {
        let marker = if scorer_name(app.scorer) == name { "> " } else { "  " };
        format!("{marker}{name:<9} {desc}")
    })
    .collect::<Vec<_>>()
    .join("\n");
    frame.render_widget(
        Paragraph::new(text).block(Block::default().title("Scorer").borders(Borders::ALL)),
        area,
    );
}

pub fn handle_key(app: &mut App, code: KeyCode) {
    if app.editing_filter {
        match code {
            KeyCode::Esc | KeyCode::Enter => app.editing_filter = false,
            KeyCode::Backspace => {
                app.rank_filter.pop();
            }
            KeyCode::Char(c) => app.rank_filter.push(c),
            _ => {}
        }
        return;
    }
    match code {
        KeyCode::Char('j') | KeyCode::Down => {
            app.rank_selection =
                (app.rank_selection + 1).min(app.visible_rank_rows().len().saturating_sub(1));
        }
        KeyCode::Char('k') | KeyCode::Up => app.rank_selection = app.rank_selection.saturating_sub(1),
        KeyCode::Char('/') => {
            app.editing_filter = true;
            app.rank_filter.clear();
        }
        KeyCode::Char('r') => app.start_rank(),
        KeyCode::Char('c') => {
            app.scorer_picker_original = Some(app.scorer);
            app.modal = Some(Modal::Scorer);
        }
        KeyCode::Char('u') => cycle_scope(app),
        KeyCode::Char('x') => app.export_rank_csv(),
        KeyCode::Enter => app.open_selected_system(),
        _ => {}
    }
}

pub fn handle_scorer_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('j') | KeyCode::Down => app.move_scorer_picker(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_scorer_picker(-1),
        KeyCode::Enter | KeyCode::Esc => app.close_scorer_picker(),
        _ => {}
    }
}

fn cycle_scope(app: &mut App) {
    match app.scope_mode {
        ScopeMode::Discovered => {
            if app.spoiler_confirmed {
                app.scope_mode = ScopeMode::All;
            } else {
                app.modal = Some(Modal::SpoilerConfirm);
            }
        }
        ScopeMode::All => app.scope_mode = ScopeMode::Discovered,
    }
}

fn scorer_name(scorer: RankScorer) -> &'static str {
    match scorer {
        RankScorer::Quick => "quick",
        RankScorer::Template => "template",
        RankScorer::Bound => "bound",
    }
}
