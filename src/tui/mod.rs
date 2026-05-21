//! Interactive TUI mode — launched when `qvm` is run with no subcommand.
//!
//! The CLI is still the source of truth for what qvm can do. This module
//! is a thin presenter on top of the same `commands::*` functions used by
//! the CLI — every action you can take in the TUI corresponds to a CLI
//! command you could have typed.
//!
//! **Keyboard only.** Mouse capture was removed because crossterm's mouse
//! escape sequences leak into the parent shell on any abnormal exit (SIGINT
//! during a child suspend(), panic during render, kernel kill, etc.). The
//! result was a shell prompt spewing `^[[<35;X;YM` on every mouse move.
//! Keyboard navigation covers everything mouse did; the trade is fine.
//!
//! Architectural rule: **this file is the only one that touches raw mode**.
//! `app.rs`, `ui.rs`, `events.rs`, `forms.rs` are pure logic / pure render
//! and have no terminal side-effects, so they unit-test cleanly.

pub mod app;
mod events;
mod forms;
pub mod onboard;
pub mod refresh;
pub mod theme;
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
use std::{
    io::stdout,
    sync::{mpsc, Arc, Mutex},
    time::Duration,
};

pub use app::{App, Mode};

/// Launch the TUI. Returns when the user presses `q` / Esc, or after a fatal
/// terminal error (in which case the terminal is restored before the error
/// propagates).
pub fn run(cfg: &Config) -> Result<()> {
    // Panic hook: any panic during render would leave the user in raw mode
    // with their cursor hidden. Both must be undone in reverse-enable order.
    // Idempotent — calling them when never enabled is a no-op.
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
    app.theme = theme::Theme::from_name(&cfg.tui.theme);
    // Initial blocking refresh so the UI is populated on first paint.
    app.refresh(cfg);
    // One-shot orphan scan; renders as a header hint if non-zero.
    app.orphan_count = crate::commands::cleanup::scan(cfg)
        .map(|v| v.len())
        .unwrap_or(0);

    // Background refresh worker. The shared selected-name lets the worker
    // refresh dominfo for whichever row the user is looking at.
    let selected: Arc<Mutex<Option<String>>> =
        Arc::new(Mutex::new(app.selected_name()));
    let (tx, rx) = mpsc::channel::<refresh::RefreshMsg>();
    let _worker = refresh::spawn(cfg.clone(), selected.clone(), tx);

    while !app.should_quit {
        terminal.draw(|f| ui::draw(f, &mut app)).map_err(io_err)?;
        app.tick = app.tick.wrapping_add(1);

        // Drain any refresh messages waiting on the channel. try_recv is
        // non-blocking so the event-poll cadence below stays responsive.
        while let Ok(msg) = rx.try_recv() {
            match msg {
                refresh::RefreshMsg::Starting    => app.is_refreshing = true,
                refresh::RefreshMsg::Result(res) => app.apply_async_refresh(res),
            }
        }

        if event::poll(Duration::from_millis(100)).map_err(io_err)? {
            if let Event::Key(k) = event::read().map_err(io_err)? {
                let action = events::map_key(&app, k);
                handle_action(&mut app, action, cfg, terminal)?;
            }
            // Non-key events (resize, paste, focus) are intentionally
            // ignored. Mouse events can't reach us — capture is disabled.
        }

        // Keep the worker's "current selection" in sync. Cheap: short lock,
        // String clone only when changed.
        if let Ok(mut g) = selected.lock() {
            let cur = app.selected_name();
            if g.as_ref() != cur.as_ref() {
                *g = cur;
            }
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
        Action::Browser => {
            if let Some(name) = app.selected_name() {
                let result = suspend(terminal, ||
                    crate::commands::vnc::run(cfg, &name, /*open*/ false, /*browser*/ true)
                );
                match result {
                    Ok(()) => app.toast_ok(format!("Closed browser bridge for '{name}'")),
                    Err(e) => app.toast_err(format!("browser '{name}' failed: {e}")),
                }
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
        Action::SubmitResize => {
            let plan = match app.take_resize_args() {
                Ok(p) => p,
                Err(msg) => {
                    app.toast_err(msg);
                    return Ok(());
                }
            };
            let res = apply_resize(cfg, terminal, &plan);
            match res {
                Ok(summary) => app.toast_ok(format!("Resized '{}': {summary}", plan.name)),
                Err(e)      => app.toast_err(format!("resize '{}' failed: {e}", plan.name)),
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

/// Release the terminal so a child process (virsh console, qemu-img convert,
/// websockify, ssh) can render normally with progress bars, then restore
/// the TUI on return.
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

/// Execute the resize plan from the inline form.
///
/// CPU + RAM changes are inline (set_cpu / set_ram both apply via `virsh
/// setvcpus --config`, take effect on next reboot — no shutdown needed).
/// Disk grow requires the VM to be stopped; `resources::resize_disk`
/// handles the stop/wait/grow flow itself but it also prompts on stdin,
/// so we suspend ratatui around the disk path.
fn apply_resize(
    cfg: &Config,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    plan: &app::ResizeArgs,
) -> crate::error::Result<String> {
    let mut applied: Vec<String> = Vec::new();

    if plan.cpus != plan.orig_cpus {
        crate::commands::resources::set_cpu(&plan.name, plan.cpus)?;
        applied.push(format!("CPU {}→{}", plan.orig_cpus, plan.cpus));
    }
    if plan.memory_gb != plan.orig_memory_gb {
        crate::commands::resources::set_ram(&plan.name, plan.memory_gb)?;
        applied.push(format!("RAM {}→{}G", plan.orig_memory_gb, plan.memory_gb));
    }
    if plan.disk_gb != plan.orig_disk_gb {
        // resize_disk shuts the VM down via a prompt on stdin — needs raw
        // mode off and the alt screen released. Restart the VM only if
        // resize_disk implicitly stopped it (which it does on demand);
        // the user can press 's' afterwards if they want it started.
        let name = plan.name.clone();
        let new = plan.disk_gb;
        let res = suspend(terminal, || {
            crate::commands::resources::resize_disk(cfg, &name, &format!("{new}G"))
        });
        res?;
        applied.push(format!("disk {}→{}G", plan.orig_disk_gb, plan.disk_gb));
    }

    if applied.is_empty() {
        Ok("nothing to change".into())
    } else {
        Ok(applied.join(", "))
    }
}

fn io_err(e: std::io::Error) -> crate::error::Error {
    crate::error::Error::User(format!("terminal error: {e}"))
}
