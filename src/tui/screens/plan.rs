use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Rect};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::tui::app::{PlanActionRow, Screen};

use super::super::app::App;
use super::selected_style;

pub fn draw(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let Some(plan) = app.plan.as_ref() else {
        frame.render_widget(
            ratatui::widgets::Paragraph::new("Open a frontier/result from Solve first.")
                .block(Block::default().title("Plan").borders(Borders::ALL)),
            area,
        );
        return;
    };
    let mut last_month = None;
    let mut rows = Vec::new();
    for (index, row) in plan.rows.iter().enumerate() {
        let month = if last_month == Some(row.month) {
            String::new()
        } else {
            last_month = Some(row.month);
            format!("month {}", row.month)
        };
        rows.push(plan_row(row, plan.checked[index], month));
    }
    let mut state = TableState::default().with_selected(Some(plan.selection));
    let table = Table::new(
        rows,
        [Constraint::Length(10), Constraint::Length(5), Constraint::Min(30)],
    )
    .block(Block::default().title(plan.header.clone()).borders(Borders::ALL))
    .row_highlight_style(selected_style())
    .highlight_symbol("> ");
    frame.render_stateful_widget(table, area, &mut state);
}

pub fn handle_key(app: &mut App, code: KeyCode) {
    let Some(plan) = app.plan.as_mut() else {
        return;
    };
    match code {
        KeyCode::Char('j') | KeyCode::Down => {
            plan.selection = (plan.selection + 1).min(plan.rows.len().saturating_sub(1));
        }
        KeyCode::Char('k') | KeyCode::Up => {
            plan.selection = plan.selection.saturating_sub(1);
        }
        KeyCode::Char(' ') => {
            if let Some(checked) = plan.checked.get_mut(plan.selection) {
                *checked = !*checked;
            }
        }
        KeyCode::Char('n') => {
            if let Some(index) = plan
                .checked
                .iter()
                .enumerate()
                .skip(plan.selection + 1)
                .find_map(|(index, checked)| (!*checked).then_some(index))
            {
                plan.selection = index;
            }
        }
        KeyCode::Char('x') => export_plan(app),
        KeyCode::Char('b') => app.active_screen = Screen::Solve,
        _ => {}
    }
}

fn plan_row(row: &PlanActionRow, checked: bool, month: String) -> Row<'_> {
    Row::new(vec![
        Cell::from(month),
        Cell::from(if checked { "[x]" } else { "[ ]" }),
        Cell::from(row.text.clone()),
    ])
}

fn export_plan(app: &mut App) {
    let Some(plan) = app.plan.as_ref() else {
        return;
    };
    let mut out = String::new();
    out.push_str(&plan.header);
    out.push('\n');
    let mut last_month = None;
    for (index, row) in plan.rows.iter().enumerate() {
        if last_month != Some(row.month) {
            out.push_str(&format!("\nmonth {}\n", row.month));
            last_month = Some(row.month);
        }
        out.push_str(&format!(
            "{} {}\n",
            if plan.checked[index] { "[x]" } else { "[ ]" },
            row.text
        ));
    }
    match std::fs::write("plan_tui.txt", out) {
        Ok(()) => app.status = "exported plan_tui.txt".to_string(),
        Err(err) => app.status = format!("plan export failed: {err}"),
    }
}
