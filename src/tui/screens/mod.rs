pub mod plan;
pub mod rank;
pub mod saves;
pub mod settings;
pub mod solve;
pub mod system;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Tabs, Wrap};
use ratatui::Frame;

use super::app::{App, Modal, Screen};

pub fn draw(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(frame, app, chunks[0]);
    draw_tabs(frame, app, chunks[1]);
    match app.active_screen {
        Screen::Saves => saves::draw(frame, app, chunks[2]),
        Screen::Rank => rank::draw(frame, app, chunks[2]),
        Screen::System => system::draw(frame, app, chunks[2]),
        Screen::Solve => solve::draw(frame, app, chunks[2]),
        Screen::Plan => plan::draw(frame, app, chunks[2]),
    }
    draw_status(frame, app, chunks[3]);
    draw_footer(frame, app, chunks[4]);

    if let Some(modal) = app.modal.clone() {
        draw_modal(frame, app, modal);
    }
}

fn draw_header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let text = format!(
        "System Solver · save: {} · credits {:.1}M · SP {} · alpha {}",
        app.active_save_label(),
        app.config.credits / 1_000_000.0,
        app.config.story_points,
        app.config.alpha_cores
    );
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::Cyan)),
        area,
    );
}

fn draw_tabs(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let titles = vec![
        Line::from("[1] Saves"),
        Line::from("[2] Rank"),
        if app.selected_system_name.is_some() {
            Line::from("[3] System")
        } else {
            Line::from(muted("[3] System"))
        },
        if app.systems.is_empty() {
            Line::from(muted("[4] Solve"))
        } else {
            Line::from("[4] Solve")
        },
        if app.plan.is_some() {
            Line::from("[5] Plan")
        } else {
            Line::from(muted("[5] Plan"))
        },
        Line::from("[s] Setup"),
    ];
    let selected = match app.active_screen {
        Screen::Saves => 0,
        Screen::Rank => 1,
        Screen::System => 2,
        Screen::Solve => 3,
        Screen::Plan => 4,
    };
    let tabs = Tabs::new(titles)
        .select(selected)
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    frame.render_widget(tabs, area);
}

fn draw_status(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let spinner = ["-", "\\", "|", "/"][app.spinner % 4];
    let text = if let Some(label) = app.job.label() {
        let elapsed = app
            .elapsed_job()
            .map(super::app::format_duration)
            .unwrap_or_else(|| "0s".to_string());
        format!(
            "{spinner} {label} · {elapsed} · {} · [x] cancel",
            app.status
        )
    } else if let Some(error) = &app.error {
        format!("error: {error}")
    } else {
        app.status.clone()
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::Yellow)),
        area,
    );
}

fn draw_footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    if matches!(app.modal, Some(Modal::Settings)) {
        let text = if app.settings_editing {
            "Type value - Enter commit - Esc cancel edit - Backspace delete"
        } else {
            "Enter edit/toggle - +/- item count - j/k move - Esc save+close"
        };
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(Color::Gray)),
            area,
        );
        return;
    }

    let text = if app.editing_filter {
        "Type to search · Enter/Esc done · Backspace delete"
    } else if app.editing_solve_param {
        "Type value · Enter commit · Esc cancel edit · Backspace delete"
    } else {
        match app.active_screen {
            Screen::Saves => "Enter activate/extract · e extract · j/k move · ? help · q quit",
            Screen::Rank => {
                "Enter inspect · c scorer · u scope · r re-rank · / filter · x export csv"
            }
            Screen::System => "b/Esc back · / search active scope · j/k planet · s solve · S setup",
            Screen::Solve => "Tab focus · Enter edit/cycle · R run · m mode · p plan · b back",
            Screen::Plan => "Space toggle · n next unchecked · x export text · b/Esc back to solve",
        }
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::Gray)),
        area,
    );
}

fn draw_modal(frame: &mut Frame<'_>, app: &mut App, modal: Modal) {
    let area = centered_rect(70, 60, frame.area());
    frame.render_widget(Clear, area);
    match modal {
        Modal::Help => {
            let text = "Global: 1 Saves · 2 Rank · 3 System · 4 Solve · 5 Plan · s Setup · ? Help · q Quit\n\
                        Move: j/k or arrows · Enter drill in · Esc back/close\n\
                        Rank: r rank · c scorer · u scope · / filter · x export CSV\n\
                        System: / jump to system · s solve this system · S setup\n\
                        Solve: Tab panes · Enter edit/cycle · R run · p plan\n\
                        Jobs: x cancels rank/solve (extract/load detach)";
            frame.render_widget(
                Paragraph::new(text)
                    .block(Block::default().title("Keymap").borders(Borders::ALL))
                    .wrap(Wrap { trim: true }),
                area,
            );
        }
        Modal::Settings => settings::draw(frame, app, area),
        Modal::Scorer => rank::draw_scorer(frame, app, area),
        Modal::SpoilerConfirm => {
            frame.render_widget(
                Paragraph::new("Show all systems? This can reveal undiscovered save content. y/n")
                    .block(
                        Block::default()
                            .title("Spoiler Guard")
                            .borders(Borders::ALL),
                    ),
                area,
            );
        }
        Modal::QuitConfirm => {
            frame.render_widget(
                Paragraph::new("A job is running. Cancel it and quit? y/n")
                    .block(Block::default().title("Quit").borders(Borders::ALL)),
                area,
            );
        }
    }
}

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

pub fn selected_style() -> Style {
    Style::default().fg(Color::Black).bg(Color::Yellow)
}

pub fn muted<'a>(text: &'a str) -> Span<'a> {
    Span::styled(text, Style::default().fg(Color::DarkGray))
}
