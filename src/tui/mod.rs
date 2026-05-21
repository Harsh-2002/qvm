//! Interactive TUI mode — launched when `qvm` is run with no subcommand.
//!
//! The CLI is still the source of truth for what qvm can do. This module
//! is a thin presenter on top of the same `commands::*` functions used by
//! the CLI — every action you can take in the TUI corresponds to a CLI
//! command you could have typed.
//!
//! Architectural rule: **this file is the only one that touches raw mode**.
//! `app.rs`, `ui.rs`, `events.rs`, `forms.rs` are pure logic / pure render
//! and have no terminal side-effects, so they unit-test cleanly.

pub mod app;
mod events;
mod forms;
mod ui;

use crate::cmd::run_tty;
use crate::config::Config;
use crate::error::Result;
use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{io::stdout, time::Duration};

pub use app::{App, Mode};

/// Launch the TUI. Returns when the user presses `q` / Esc, or after a fatal
/// terminal error (in which case the terminal is restored before the error
/// propagates).
pub fn run(cfg: &Config) -> Result<()> {
    // Panic hook: any panic during render would leave the user in raw mode
    // with their cursor hidden. Install a hook that ALWAYS restores the
    // terminal first, then chains to the previous hook.
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen, Show);
        original(info);
    }));

    enable_raw_mode().map_err(io_err)?;
    execute!(stdout(), EnterAlternateScreen, Hide).map_err(io_err)?;

    let mut terminal = Terminal::new(CrosstermBackend::new(stdout())).map_err(io_err)?;
    let result = main_loop(&mut terminal, cfg);

    // Always restore the terminal, even on error.
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen, Show);

    result
}

fn main_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    cfg: &Config,
) -> Result<()> {
    let mut app = App::new();
    app.refresh(cfg);

    while !app.should_quit {
        terminal.draw(|f| ui::draw(f, &app)).map_err(io_err)?;

        if event::poll(Duration::from_millis(200)).map_err(io_err)? {
            let ev = event::read().map_err(io_err)?;
            if let Event::Key(k) = ev {
                let action = events::map_key(&app, k);
                handle_action(&mut app, action, cfg, terminal)?;
            }
        }

        if app.tick_due() {
            app.refresh(cfg);
        }
    }
    Ok(())
}

/// Dispatch an action returned from key mapping. Most actions are state-only
/// (handled by `App::apply`); the ones that need raw-mode suspension
/// (create, console) live here because `mod.rs` owns terminal state.
fn handle_action(
    app: &mut App,
    action: events::Action,
    cfg: &Config,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
) -> Result<()> {
    use events::Action;

    match action {
        Action::Console => {
            if let Some(name) = app.selected_name() {
                // virsh console is interactive — needs inherited stdin to read
                // keypresses. run_inherit's Stdio::null on stdin gave a fatal
                // "Cannot run interactive console without a controlling TTY".
                suspend(terminal, || {
                    println!("(Press Ctrl-] to leave the console and return to qvm.)");
                    run_tty("virsh", ["console", &name])
                })?;
                app.toast_ok(format!("Returned from console of '{name}'"));
                app.refresh(cfg);
            }
        }
        Action::SubmitCreate => {
            let args = match app.take_create_args() {
                Ok(a) => a,
                Err(msg) => {
                    app.toast_err(msg);
                    return Ok(());
                }
            };
            let name = args.name.clone();
            let res = suspend(terminal, || crate::commands::create::run(cfg, args));
            match res {
                Ok(()) => app.toast_ok(format!("Created '{name}'")),
                Err(e) => app.toast_err(format!("create '{name}' failed: {e}")),
            }
            app.refresh(cfg);
        }
        other => {
            // Pure state mutations: delete confirm, start/stop/restart, mode
            // changes, navigation, filter, sort, help, etc.
            app.apply(other, cfg);
        }
    }
    Ok(())
}

/// Release the terminal so a child process (virsh console, qemu-img convert)
/// can render normally with progress bars, then restore the TUI on return.
fn suspend<F, R>(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    f: F,
) -> R
where
    F: FnOnce() -> R,
{
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen, Show);
    let r = f();
    let _ = enable_raw_mode();
    let _ = execute!(stdout(), EnterAlternateScreen, Hide);
    let _ = terminal.clear();
    r
}

fn io_err(e: std::io::Error) -> crate::error::Error {
    crate::error::Error::User(format!("terminal error: {e}"))
}
