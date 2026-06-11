pub mod app;
pub mod config;
pub mod jobs;
mod screens;

use std::io::{self, stdout};
use std::panic;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use self::app::{App, Modal, ScopeMode, Screen};
use self::config::{TuiConfig, CONFIG_PATH};

type PanicHook = Box<dyn Fn(&panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

pub fn run() -> io::Result<()> {
    let (config, status) = TuiConfig::load(CONFIG_PATH);
    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;

    let mut app = App::new(config, status);
    app.start_initial_load();

    while !app.should_quit {
        app.tick();
        terminal.draw(|frame| screens::draw(frame, &mut app))?;
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(&mut app, key.code);
                }
            }
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode) {
    if let Some(modal) = app.modal.clone() {
        match modal {
            Modal::Help => {
                app.modal = None;
                return;
            }
            Modal::QuitConfirm => match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    app.cancel_job();
                    app.should_quit = true;
                    return;
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    app.modal = None;
                    return;
                }
                _ => return,
            },
            Modal::SpoilerConfirm => match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    app.spoiler_confirmed = true;
                    app.scope_mode = ScopeMode::All;
                    app.modal = None;
                    app.status = "scope: all systems".to_string();
                    return;
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    app.modal = None;
                    return;
                }
                _ => return,
            },
            Modal::Scorer => {
                screens::rank::handle_scorer_key(app, code);
                return;
            }
            Modal::Settings => {
                screens::settings::handle_key(app, code);
                return;
            }
        }
    }

    if app.editing_filter || app.editing_solve_param {
        route_screen_key(app, code);
        return;
    }

    if app.active_screen == Screen::System
        && matches!(code, KeyCode::Char('s') | KeyCode::Char('S'))
    {
        route_screen_key(app, code);
        return;
    }

    match code {
        KeyCode::Char('q') => {
            if app.job.is_running() {
                app.modal = Some(Modal::QuitConfirm);
            } else {
                app.should_quit = true;
            }
        }
        KeyCode::Char('?') => app.modal = Some(Modal::Help),
        KeyCode::Char('s') => app.modal = Some(Modal::Settings),
        KeyCode::Char('1') => app.active_screen = Screen::Saves,
        KeyCode::Char('2') => {
            app.active_screen = Screen::Rank;
            app.maybe_auto_rank();
        }
        KeyCode::Char('3') => {
            if app.selected_system_name.is_some() {
                app.active_screen = Screen::System;
            } else {
                app.status = "open a ranked system first".to_string();
            }
        }
        KeyCode::Char('4') => {
            if app.systems.is_empty() {
                app.status = "load a save's systems first".to_string();
            } else {
                app.active_screen = Screen::Solve;
                app.restore_solve_cache();
            }
        }
        KeyCode::Char('5') => {
            if app.plan.is_some() {
                app.active_screen = Screen::Plan;
            } else {
                app.status = "open a plan from Solve first".to_string();
            }
        }
        KeyCode::Char('x') if app.job.is_running() => app.cancel_job(),
        KeyCode::Esc => match app.active_screen {
            Screen::Saves => {}
            Screen::Rank => app.active_screen = Screen::Saves,
            Screen::System => app.active_screen = Screen::Rank,
            Screen::Solve => app.active_screen = Screen::System,
            Screen::Plan => app.active_screen = Screen::Solve,
        },
        _ => route_screen_key(app, code),
    }
}

fn route_screen_key(app: &mut App, code: KeyCode) {
    match app.active_screen {
        Screen::Saves => screens::saves::handle_key(app, code),
        Screen::Rank => screens::rank::handle_key(app, code),
        Screen::System => screens::system::handle_key(app, code),
        Screen::Solve => screens::solve::handle_key(app, code),
        Screen::Plan => screens::plan::handle_key(app, code),
    }
}

struct TerminalGuard {
    previous_hook: Option<PanicHook>,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen)?;
        let previous_hook = panic::take_hook();
        panic::set_hook(Box::new(|info| {
            let _ = disable_raw_mode();
            let _ = execute!(stdout(), LeaveAlternateScreen);
            eprintln!("{info}");
        }));
        Ok(Self {
            previous_hook: Some(previous_hook),
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        if let Some(hook) = self.previous_hook.take() {
            panic::set_hook(hook);
        }
    }
}
