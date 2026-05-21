//! Step-by-step onboarding wizard.
//!
//! Triggered when `qvm` is launched with no subcommand AND
//! `/etc/qvm/config.toml` does not exist. Replaces the old
//! "Config not found. Run: sudo qvm init" error with a guided seven-
//! screen flow that hands a fresh host from "qvm just installed" to
//! "first VM running" without gaps.
//!
//! The non-interactive `qvm init` command is unchanged for scripts.
//!
//! On success this writes `cfg_path` and returns `Ok(())`. The caller
//! (`main.rs`) then proceeds into the main TUI.

use crate::cmd::have;
use crate::commands::doctor::DEPS;
use crate::commands::init::{render_config, WizardAnswers};
use crate::commands::pull;
use crate::config::{builtin_distros, Config};
use crate::error::{Error, Result};
use crate::tui::forms::TextInput;
use crate::tui::theme::Theme;

use crossterm::{
    cursor::{Hide, Show},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame, Terminal,
    backend::CrosstermBackend,
};
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::time::Duration;

const STEPS: &[&str] = &[
    "Welcome", "Host check", "Network", "SSH keys",
    "Storage", "First image", "Done",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepKind {
    Welcome     = 0,
    HostCheck   = 1,
    Network     = 2,
    SshKeys     = 3,
    Paths       = 4,
    FirstImage  = 5,
    Done        = 6,
}

impl StepKind {
    fn next(self) -> Self {
        use StepKind::*;
        match self {
            Welcome    => HostCheck,
            HostCheck  => Network,
            Network    => SshKeys,
            SshKeys    => Paths,
            Paths      => FirstImage,
            FirstImage => Done,
            Done       => Done,
        }
    }
    fn prev(self) -> Self {
        use StepKind::*;
        match self {
            Welcome    => Welcome,
            HostCheck  => Welcome,
            Network    => HostCheck,
            SshKeys    => Network,
            Paths      => SshKeys,
            FirstImage => Paths,
            Done       => FirstImage,
        }
    }
    fn idx(self) -> usize { self as usize }
}

struct OnboardApp {
    step: StepKind,
    theme: Theme,
    should_quit: bool,
    /// Set once the user has finished — `run` returns Ok with the
    /// config path written; `main` then loads + launches the TUI.
    finished: bool,

    // Step state.
    bridge_choices: Vec<String>,
    bridge_sel:     usize,
    bridge_custom:  TextInput,
    use_custom_bridge: bool,

    detected_keys: Vec<String>,
    key_selected:  Vec<bool>,
    paste_buf:     TextInput,

    images_path:    TextInput,
    vms_path:       TextInput,
    cloudinit_path: TextInput,
    paths_field:    usize, // 0..3

    distro_choices: Vec<String>,
    distro_sel:     usize,
    skip_first_pull: bool,

    /// Last action's outcome — bottom-bar message until cleared.
    flash: Option<(String, bool /* ok? */)>,
}

impl OnboardApp {
    fn new() -> Self {
        let bridges = detect_bridges();
        let keys    = detect_ssh_keys();
        let key_selected = vec![true; keys.len()]; // pre-checked
        let distros: Vec<String> = builtin_distros().keys().cloned().collect();
        let debian_idx = distros.iter().position(|d| d == "debian:13").unwrap_or(0);

        Self {
            step: StepKind::Welcome,
            theme: Theme::default(),
            should_quit: false,
            finished: false,

            bridge_choices: bridges,
            bridge_sel:     0,
            bridge_custom:  TextInput::with_value("br0"),
            use_custom_bridge: false,

            detected_keys: keys,
            key_selected,
            paste_buf:     TextInput::default(),

            images_path:    TextInput::with_value("/var/lib/qvm/images"),
            vms_path:       TextInput::with_value("/var/lib/qvm/vms"),
            cloudinit_path: TextInput::with_value("/var/lib/qvm/cloudinit"),
            paths_field:    0,

            distro_choices: distros,
            distro_sel:     debian_idx,
            skip_first_pull: false,

            flash: None,
        }
    }

    fn bridge(&self) -> String {
        if self.use_custom_bridge || self.bridge_choices.is_empty() {
            self.bridge_custom.value.trim().to_string()
        } else {
            self.bridge_choices[self.bridge_sel].clone()
        }
    }

    fn selected_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.detected_keys.iter().enumerate()
            .filter(|(i, _)| self.key_selected.get(*i).copied().unwrap_or(false))
            .map(|(_, k)| k.clone())
            .collect();
        for extra in self.paste_buf.value.lines() {
            let t = extra.trim();
            if !t.is_empty() && !keys.contains(&t.to_string()) {
                keys.push(t.to_string());
            }
        }
        keys
    }
}

// ── entry point ──────────────────────────────────────────────────────────────

pub fn run(cfg_path: &Path) -> Result<()> {
    // Install the same panic hook as the main TUI so a crash never leaves
    // the terminal in raw mode.
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen, Show);
        original(info);
    }));

    enable_raw_mode().map_err(io_err)?;
    execute!(stdout(), EnterAlternateScreen, Hide).map_err(io_err)?;

    let mut terminal = Terminal::new(CrosstermBackend::new(stdout())).map_err(io_err)?;
    let result = main_loop(&mut terminal, cfg_path);

    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen, Show);

    result
}

fn main_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    cfg_path: &Path,
) -> Result<()> {
    let mut app = OnboardApp::new();
    while !app.should_quit && !app.finished {
        terminal.draw(|f| draw(f, &app)).map_err(io_err)?;
        if event::poll(Duration::from_millis(150)).map_err(io_err)? {
            if let Event::Key(k) = event::read().map_err(io_err)? {
                handle_key(&mut app, k, cfg_path, terminal)?;
            }
        }
    }
    if app.should_quit {
        return Err(Error::User("onboarding cancelled".into()));
    }
    Ok(())
}

fn io_err(e: std::io::Error) -> Error {
    Error::User(format!("terminal error: {e}"))
}

/// Release the terminal so a child process can render normally with
/// progress bars, then restore the wizard.
fn suspend<F, R>(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    f: F,
) -> R
where F: FnOnce() -> R {
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen, Show);
    let r = f();
    let _ = enable_raw_mode();
    let _ = execute!(stdout(), EnterAlternateScreen, Hide);
    let _ = terminal.clear();
    r
}

// ── event handling ───────────────────────────────────────────────────────────

fn handle_key(
    app: &mut OnboardApp,
    k: KeyEvent,
    cfg_path: &Path,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
) -> Result<()> {
    // Global: Ctrl-C quits.
    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
        app.should_quit = true;
        return Ok(());
    }
    // Esc: go back (unless at first step, then quit).
    if k.code == KeyCode::Esc {
        if app.step == StepKind::Welcome {
            app.should_quit = true;
        } else {
            app.step = app.step.prev();
            app.flash = None;
        }
        return Ok(());
    }

    match app.step {
        StepKind::Welcome => {
            if k.code == KeyCode::Enter { app.step = app.step.next(); }
        }
        StepKind::HostCheck => match k.code {
            KeyCode::Enter        => { app.step = app.step.next(); }
            KeyCode::Char('i') | KeyCode::Char('I') => {
                let cmd = build_install_command();
                if let Some(c) = cmd {
                    suspend(terminal, || {
                        println!("Running: {c}");
                        let _ = std::process::Command::new("sh").arg("-c").arg(&c).status();
                    });
                    app.flash = Some(("Re-checked. Press Enter to continue.".into(), true));
                } else {
                    app.flash = Some(("Could not determine package manager.".into(), false));
                }
            }
            _ => {}
        },
        StepKind::Network => match k.code {
            KeyCode::Down | KeyCode::Char('j') => {
                if !app.bridge_choices.is_empty() && !app.use_custom_bridge {
                    let n = app.bridge_choices.len() + 1; // +1 for "custom" slot
                    let cur = app.bridge_sel;
                    let next = (cur + 1) % n;
                    if next == app.bridge_choices.len() {
                        app.use_custom_bridge = true;
                    } else {
                        app.bridge_sel = next;
                    }
                } else if !app.bridge_choices.is_empty() && app.use_custom_bridge {
                    app.use_custom_bridge = false;
                    app.bridge_sel = 0;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if !app.bridge_choices.is_empty() && app.use_custom_bridge {
                    app.use_custom_bridge = false;
                    app.bridge_sel = app.bridge_choices.len().saturating_sub(1);
                } else if !app.bridge_choices.is_empty() {
                    let n = app.bridge_choices.len() + 1;
                    app.bridge_sel = (app.bridge_sel + n - 1) % n;
                    if app.bridge_sel == app.bridge_choices.len() {
                        app.use_custom_bridge = true;
                    }
                }
            }
            KeyCode::Char(c) if app.use_custom_bridge || app.bridge_choices.is_empty() => {
                app.bridge_custom.insert(c);
            }
            KeyCode::Backspace if app.use_custom_bridge || app.bridge_choices.is_empty() => {
                app.bridge_custom.backspace();
            }
            KeyCode::Enter => {
                if app.bridge().is_empty() {
                    app.flash = Some(("Bridge name cannot be empty.".into(), false));
                } else {
                    app.step = app.step.next();
                }
            }
            _ => {}
        },
        StepKind::SshKeys => match k.code {
            KeyCode::Char(' ') => {
                // Bulk-toggle all detected keys. Selection is all-on or
                // all-off — power users add specific keys via the paste box.
                let any_off = app.key_selected.iter().any(|b| !b);
                for b in &mut app.key_selected { *b = any_off; }
            }
            // The `y` override path comes BEFORE generic Char(c) so it
            // wins when the user is dismissing the "no keys" warning.
            KeyCode::Char('y') | KeyCode::Char('Y')
                if app.selected_keys().is_empty() && app.flash.is_some() => {
                    app.step = app.step.next();
                    app.flash = None;
            }
            KeyCode::Char(c)   => app.paste_buf.insert(c),
            KeyCode::Backspace => app.paste_buf.backspace(),
            KeyCode::Enter => {
                if app.selected_keys().is_empty() {
                    app.flash = Some((
                        "No SSH keys selected. You won't be able to ssh in. Press [y] to continue anyway.".into(),
                        false,
                    ));
                } else {
                    app.step = app.step.next();
                }
            }
            _ => {}
        },
        StepKind::Paths => match k.code {
            KeyCode::Tab       => { app.paths_field = (app.paths_field + 1) % 3; }
            KeyCode::BackTab   => { app.paths_field = (app.paths_field + 2) % 3; }
            KeyCode::Backspace => { focused_path(app).backspace(); }
            KeyCode::Delete    => { focused_path(app).delete(); }
            KeyCode::Left      => { focused_path(app).left(); }
            KeyCode::Right     => { focused_path(app).right(); }
            KeyCode::Home      => { focused_path(app).home(); }
            KeyCode::End       => { focused_path(app).end(); }
            KeyCode::Char(c)   => { focused_path(app).insert(c); }
            KeyCode::Enter     => { app.step = app.step.next(); }
            _ => {}
        },
        StepKind::FirstImage => match k.code {
            KeyCode::Down | KeyCode::Char('j') if !app.distro_choices.is_empty() => {
                app.distro_sel = (app.distro_sel + 1) % app.distro_choices.len();
            }
            KeyCode::Up | KeyCode::Char('k') if !app.distro_choices.is_empty() => {
                let n = app.distro_choices.len();
                app.distro_sel = (app.distro_sel + n - 1) % n;
            }
            KeyCode::Char(' ') => { app.skip_first_pull = !app.skip_first_pull; }
            KeyCode::Enter => {
                if !app.skip_first_pull {
                    // Suspend ratatui so the streaming download's progress
                    // bar (from pull::pull_one) renders cleanly.
                    let distro = app.distro_choices[app.distro_sel].clone();
                    let cfg = build_partial_cfg(app);
                    let result = suspend(terminal, || {
                        println!("Pulling {distro}...");
                        pull::pull_one(&cfg, &distro)
                    });
                    match result {
                        Ok(()) => {
                            app.flash = Some((format!("Pulled '{distro}'. Press Enter to continue."), true));
                            // Move on automatically.
                            app.step = app.step.next();
                        }
                        Err(e) => {
                            app.flash = Some((format!("Pull failed: {e}. Press Space to skip."), false));
                        }
                    }
                } else {
                    app.step = app.step.next();
                }
            }
            _ => {}
        },
        StepKind::Done => {
            if k.code == KeyCode::Enter {
                // Write the config and finish.
                let toml = render_config(WizardAnswers {
                    bridge:         &app.bridge(),
                    distro:         &app.distro_choices.get(app.distro_sel).cloned().unwrap_or_default(),
                    cpus:           2,
                    memory_gb:      4,
                    disk_gb:        50,
                    autostart:      true,
                    grub_timeout:   0,
                    vnc_bind:       "127.0.0.1",
                    ssh_keys:       &app.selected_keys(),
                    images_path:    &app.images_path.value,
                    vms_path:       &app.vms_path.value,
                    cloudinit_path: &app.cloudinit_path.value,
                });
                if let Some(parent) = cfg_path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| Error::User(format!("create config dir: {e}")))?;
                }
                std::fs::write(cfg_path, toml)
                    .map_err(|e| Error::User(format!("write config: {e}")))?;
                // Make sure the data dirs exist too so first `qvm run` works.
                let _ = std::fs::create_dir_all(&app.images_path.value);
                let _ = std::fs::create_dir_all(&app.vms_path.value);
                let _ = std::fs::create_dir_all(&app.cloudinit_path.value);
                app.finished = true;
            }
        }
    }
    Ok(())
}

fn focused_path(app: &mut OnboardApp) -> &mut TextInput {
    match app.paths_field {
        0 => &mut app.images_path,
        1 => &mut app.vms_path,
        _ => &mut app.cloudinit_path,
    }
}

// ── render ───────────────────────────────────────────────────────────────────

fn draw(f: &mut Frame, app: &OnboardApp) {
    let area = f.area();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),   // progress + step title
            Constraint::Min(5),      // step body
            Constraint::Length(3),   // footer hints
        ])
        .split(area);

    draw_progress(f, layout[0], app);
    draw_step(f, layout[1], app);
    draw_footer(f, layout[2], app);
}

fn draw_progress(f: &mut Frame, area: Rect, app: &OnboardApp) {
    let t = &app.theme;
    let cur = app.step.idx();
    // Progress strip: ● for completed/current, ○ for pending.
    let mut dots: Vec<Span<'_>> = Vec::new();
    for i in 0..STEPS.len() {
        let style = if i < cur {
            Style::default().fg(t.ok)
        } else if i == cur {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.text_faint)
        };
        let glyph = if i < cur { "●" } else if i == cur { "◉" } else { "○" };
        dots.push(Span::styled(glyph.to_string(), style));
        if i + 1 < STEPS.len() {
            dots.push(Span::styled(" ── ", Style::default().fg(t.text_faint)));
        }
    }
    let title = format!("Step {} of {} · {}", cur + 1, STEPS.len(), STEPS[cur]);

    f.render_widget(
        Paragraph::new(vec![
            Line::from(dots).alignment(Alignment::Center),
            Line::from(Span::styled(title,
                Style::default().fg(t.text).add_modifier(Modifier::BOLD)))
                .alignment(Alignment::Center),
        ]),
        area.inner(Margin { horizontal: 0, vertical: 1 }),
    );
}

fn draw_step(f: &mut Frame, area: Rect, app: &OnboardApp) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(app.theme.border));
    let inner = block.inner(area);
    f.render_widget(block, area);
    match app.step {
        StepKind::Welcome    => draw_welcome(f, inner, app),
        StepKind::HostCheck  => draw_host_check(f, inner, app),
        StepKind::Network    => draw_network(f, inner, app),
        StepKind::SshKeys    => draw_ssh_keys(f, inner, app),
        StepKind::Paths      => draw_paths(f, inner, app),
        StepKind::FirstImage => draw_first_image(f, inner, app),
        StepKind::Done       => draw_done(f, inner, app),
    }
}

fn draw_welcome(f: &mut Frame, area: Rect, app: &OnboardApp) {
    let t = &app.theme;
    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled("Welcome to qvm.",
            Style::default().fg(t.text).add_modifier(Modifier::BOLD))).alignment(Alignment::Center),
        Line::raw(""),
        Line::from(Span::styled(
            "qvm is a thin, opinionated CLI + TUI for managing KVM/libvirt",
            Style::default().fg(t.text_dim))).alignment(Alignment::Center),
        Line::from(Span::styled(
            "virtual machines on this host. No daemon. Self-contained disks.",
            Style::default().fg(t.text_dim))).alignment(Alignment::Center),
        Line::raw(""),
        Line::from(Span::styled(
            "We'll set up in 6 quick steps. You can press Esc at any time to go back.",
            Style::default().fg(t.text_dim))).alignment(Alignment::Center),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Press ", Style::default().fg(t.text_dim)),
            Span::styled("[Enter]", Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled(" to begin.", Style::default().fg(t.text_dim)),
        ]).alignment(Alignment::Center),
    ];
    centered_paragraph(f, area, lines);
}

fn draw_host_check(f: &mut Frame, area: Rect, app: &OnboardApp) {
    let t = &app.theme;
    let mut lines: Vec<Line<'_>> = vec![
        Line::from(Span::styled("Checking host dependencies…",
            Style::default().fg(t.text_dim))),
        Line::raw(""),
    ];
    let mut missing_any = false;
    for d in DEPS {
        let present = have(d.binary);
        if !present { missing_any = true; }
        let glyph = if present { "✓" } else { "✗" };
        let glyph_style = if present {
            Style::default().fg(t.ok).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.err).add_modifier(Modifier::BOLD)
        };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(glyph, glyph_style),
            Span::raw(" "),
            Span::styled(format!("{:<18}", d.binary),
                Style::default().fg(t.text)),
            Span::styled(d.why, Style::default().fg(t.text_dim)),
        ]));
    }
    lines.push(Line::raw(""));
    if missing_any {
        lines.push(Line::from(Span::styled(
            "Some tools are missing. Press [i] to install them all, or",
            Style::default().fg(t.warn))));
        lines.push(Line::from(Span::styled(
            "[Enter] to continue anyway (you'll need to fix these before creating a VM).",
            Style::default().fg(t.warn))));
    } else {
        lines.push(Line::from(Span::styled("All dependencies present.",
            Style::default().fg(t.ok).add_modifier(Modifier::BOLD))));
        lines.push(Line::from(Span::styled("Press [Enter] to continue.",
            Style::default().fg(t.text_dim))));
    }
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        area.inner(Margin { horizontal: 2, vertical: 1 }),
    );
}

fn draw_network(f: &mut Frame, area: Rect, app: &OnboardApp) {
    let t = &app.theme;
    let mut lines: Vec<Line<'_>> = vec![
        Line::from(Span::styled(
            "Pick the network bridge new VMs will attach to.",
            Style::default().fg(t.text_dim))),
        Line::from(Span::styled(
            "Use ↑/↓ to move, Enter to confirm. Type to edit a custom name.",
            Style::default().fg(t.text_faint))),
        Line::raw(""),
    ];
    if app.bridge_choices.is_empty() {
        lines.push(Line::from(Span::styled(
            "No bridges detected on this host. Enter one below (default: br0):",
            Style::default().fg(t.warn))));
        lines.push(Line::raw(""));
    }
    for (i, b) in app.bridge_choices.iter().enumerate() {
        let selected = i == app.bridge_sel && !app.use_custom_bridge;
        let marker = if selected { "▶" } else { " " };
        let style = if selected {
            Style::default().fg(t.text).bg(t.surface).add_modifier(Modifier::BOLD)
        } else { Style::default().fg(t.text) };
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(marker, Style::default().fg(t.accent)),
            Span::raw("  "),
            Span::styled(b.clone(), style),
        ]));
    }
    // Custom row
    let cur_custom = app.use_custom_bridge || app.bridge_choices.is_empty();
    let marker = if cur_custom { "▶" } else { " " };
    let val = app.bridge_custom.value.clone();
    lines.push(Line::from(vec![
        Span::raw(" "),
        Span::styled(marker, Style::default().fg(t.accent)),
        Span::raw("  "),
        Span::styled("custom: ", Style::default().fg(t.text_dim)),
        Span::styled(val,
            if cur_custom {
                Style::default().fg(t.text).add_modifier(Modifier::BOLD)
            } else { Style::default().fg(t.text_dim) }),
    ]));
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        area.inner(Margin { horizontal: 2, vertical: 1 }),
    );
}

fn draw_ssh_keys(f: &mut Frame, area: Rect, app: &OnboardApp) {
    let t = &app.theme;
    let mut lines: Vec<Line<'_>> = vec![
        Line::from(Span::styled(
            "SSH public keys to install in every new VM (login user + root).",
            Style::default().fg(t.text_dim))),
        Line::from(Span::styled(
            "[Space] toggles detected keys · type to paste more · Enter when done",
            Style::default().fg(t.text_faint))),
        Line::raw(""),
    ];
    if app.detected_keys.is_empty() {
        lines.push(Line::from(Span::styled(
            "No keys found in ~/.ssh/ or /home/$SUDO_USER/.ssh/. Paste below:",
            Style::default().fg(t.warn))));
    } else {
        lines.push(Line::from(Span::styled("Detected:",
            Style::default().fg(t.text_dim))));
        for (i, k) in app.detected_keys.iter().enumerate() {
            let on = app.key_selected.get(i).copied().unwrap_or(false);
            let mark = if on { "[x]" } else { "[ ]" };
            let mark_style = if on {
                Style::default().fg(t.ok).add_modifier(Modifier::BOLD)
            } else { Style::default().fg(t.text_faint) };
            // Truncate long keys to fit.
            let preview: String = if k.chars().count() > 70 {
                k.chars().take(67).collect::<String>() + "..."
            } else { k.clone() };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(mark, mark_style),
                Span::raw(" "),
                Span::styled(preview, Style::default().fg(t.text)),
            ]));
        }
        lines.push(Line::raw(""));
    }
    lines.push(Line::from(Span::styled(
        "Paste extra keys here (one per line; Backspace to edit):",
        Style::default().fg(t.text_dim))));
    let paste_show = if app.paste_buf.value.is_empty() {
        "(empty)".to_string()
    } else { app.paste_buf.value.clone() };
    lines.push(Line::from(Span::styled(paste_show,
        Style::default().fg(t.text))));
    if let Some((msg, ok)) = &app.flash {
        let style = if *ok { Style::default().fg(t.ok) } else { Style::default().fg(t.warn) };
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(msg.clone(), style)));
    }
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        area.inner(Margin { horizontal: 2, vertical: 1 }),
    );
}

fn draw_paths(f: &mut Frame, area: Rect, app: &OnboardApp) {
    let t = &app.theme;
    let inner = area.inner(Margin { horizontal: 2, vertical: 1 });
    let labels = ["Base images", "VM disks", "Cloud-init seeds"];
    let inputs = [&app.images_path, &app.vms_path, &app.cloudinit_path];
    let mut lines: Vec<Line<'_>> = vec![
        Line::from(Span::styled(
            "Where qvm keeps its files. Defaults are fine for most homelabs.",
            Style::default().fg(t.text_dim))),
        Line::from(Span::styled(
            "Tab moves between fields · Enter to continue",
            Style::default().fg(t.text_faint))),
        Line::raw(""),
    ];
    for i in 0..3 {
        let focused = i == app.paths_field;
        let marker = if focused { "▸" } else { " " };
        let label_style = if focused {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else { Style::default().fg(t.text_dim) };
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(marker, Style::default().fg(t.accent)),
            Span::raw(" "),
            Span::styled(format!("{:<20}", labels[i]), label_style),
            Span::styled(inputs[i].value.clone(),
                Style::default().fg(t.text)),
        ]));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_first_image(f: &mut Frame, area: Rect, app: &OnboardApp) {
    let t = &app.theme;
    let mut lines: Vec<Line<'_>> = vec![
        Line::from(Span::styled(
            "Pull a base image now so your first VM can launch instantly.",
            Style::default().fg(t.text_dim))),
        Line::from(Span::styled(
            "↑/↓ to choose · Enter to pull (~500 MB) · Space to skip — pull later with `qvm pull <distro>`",
            Style::default().fg(t.text_faint))),
        Line::raw(""),
    ];
    for (i, d) in app.distro_choices.iter().enumerate() {
        let sel = i == app.distro_sel;
        let marker = if sel { "▶" } else { " " };
        let style = if sel {
            Style::default().fg(t.text).bg(t.surface).add_modifier(Modifier::BOLD)
        } else { Style::default().fg(t.text) };
        lines.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(marker, Style::default().fg(t.accent)),
            Span::raw("  "),
            Span::styled(d.clone(), style),
        ]));
    }
    lines.push(Line::raw(""));
    let skip_label = if app.skip_first_pull {
        Span::styled("[x] Skip for now",
            Style::default().fg(t.warn).add_modifier(Modifier::BOLD))
    } else {
        Span::styled("[ ] Skip for now",
            Style::default().fg(t.text_dim))
    };
    lines.push(Line::from(vec![Span::raw("  "), skip_label]));
    if let Some((msg, ok)) = &app.flash {
        let style = if *ok { Style::default().fg(t.ok) } else { Style::default().fg(t.err) };
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(msg.clone(), style)));
    }
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        area.inner(Margin { horizontal: 2, vertical: 1 }),
    );
}

fn draw_done(f: &mut Frame, area: Rect, app: &OnboardApp) {
    let t = &app.theme;
    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled("qvm is ready.",
            Style::default().fg(t.ok).add_modifier(Modifier::BOLD))).alignment(Alignment::Center),
        Line::raw(""),
        Line::from(Span::styled(
            "On [Enter], the config will be written to /etc/qvm/config.toml and the main TUI will open.",
            Style::default().fg(t.text_dim))).alignment(Alignment::Center),
        Line::raw(""),
        Line::from(Span::styled("Quick-start:",
            Style::default().fg(t.text).add_modifier(Modifier::BOLD))).alignment(Alignment::Center),
        Line::from(Span::styled("  sudo qvm                            # opens the TUI (what you'll see next)",
            Style::default().fg(t.text))).alignment(Alignment::Center),
        Line::from(Span::styled("  sudo qvm run my-vm debian:13 -u me -p secret",
            Style::default().fg(t.text))).alignment(Alignment::Center),
        Line::from(Span::styled("  sudo qvm vnc my-vm --browser        # opens a noVNC bridge + QR",
            Style::default().fg(t.text))).alignment(Alignment::Center),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Press ", Style::default().fg(t.text_dim)),
            Span::styled("[Enter]", Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled(" to finish.", Style::default().fg(t.text_dim)),
        ]).alignment(Alignment::Center),
    ];
    centered_paragraph(f, area, lines);
}

fn draw_footer(f: &mut Frame, area: Rect, app: &OnboardApp) {
    let t = &app.theme;
    let hint = match app.step {
        StepKind::Welcome    => "  [Enter] Begin   [Ctrl-C] Quit",
        StepKind::HostCheck  => "  [Enter] Continue   [i] Install missing   [Esc] Back",
        StepKind::Network    => "  [Enter] Continue   [↑/↓] Choose   type for custom   [Esc] Back",
        StepKind::SshKeys    => "  [Space] Toggle detected   type to paste   [Enter] Continue   [Esc] Back",
        StepKind::Paths      => "  [Tab] Move   [Enter] Continue   [Esc] Back",
        StepKind::FirstImage => "  [↑/↓] Choose   [Enter] Pull   [Space] Skip   [Esc] Back",
        StepKind::Done       => "  [Enter] Finish   [Esc] Back",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border));
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(hint,
            Style::default().fg(t.text_dim)))),
        inner,
    );
}

fn centered_paragraph(f: &mut Frame, area: Rect, lines: Vec<Line<'_>>) {
    let total = lines.len() as u16;
    let pad_top = area.height.saturating_sub(total) / 2;
    let r = Rect {
        x: area.x,
        y: area.y + pad_top,
        width: area.width,
        height: total.min(area.height),
    };
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), r);
}

// ── helpers (host detection) ─────────────────────────────────────────────────

fn detect_bridges() -> Vec<String> {
    let Ok(rd) = std::fs::read_dir("/sys/class/net") else { return vec![]; };
    let mut out: Vec<String> = rd.filter_map(|e| e.ok())
        .filter(|e| e.path().join("bridge").is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

fn detect_ssh_keys() -> Vec<String> {
    let mut homes: Vec<PathBuf> = Vec::new();
    if let Some(h) = std::env::var_os("HOME") { homes.push(PathBuf::from(h)); }
    if let Some(u) = std::env::var_os("SUDO_USER") {
        homes.push(PathBuf::from(format!("/home/{}", u.to_string_lossy())));
    }
    homes.dedup();
    let mut out: Vec<String> = Vec::new();
    for home in homes {
        let ssh = home.join(".ssh");
        let Ok(rd) = std::fs::read_dir(&ssh) else { continue; };
        for e in rd.flatten() {
            let name = e.file_name();
            let ns = name.to_string_lossy();
            if !ns.ends_with(".pub") { continue; }
            if let Ok(s) = std::fs::read_to_string(e.path()) {
                let s = s.trim().to_string();
                if !s.is_empty() && !out.contains(&s) { out.push(s); }
            }
        }
    }
    out
}

fn build_install_command() -> Option<String> {
    // Reuse doctor's package map. We only generate a hint string;
    // execution is via `sh -c`.
    use crate::commands::doctor::Family;
    let fam = Family::from_os_release();
    let install_cmd = fam.install_cmd()?;
    let mut pkgs: Vec<&str> = Vec::new();
    for d in DEPS {
        if !have(d.binary) {
            for p in fam.packages(&d.packages).split_whitespace() {
                if !pkgs.contains(&p) { pkgs.push(p); }
            }
        }
    }
    if pkgs.is_empty() { return None; }
    Some(format!("{install_cmd} {}", pkgs.join(" ")))
}

/// Build a minimal Config from current onboarding state so `pull::pull_one`
/// can resolve image paths during step 6.
fn build_partial_cfg(app: &OnboardApp) -> Config {
    let mut cfg = Config::default();
    cfg.paths.images    = PathBuf::from(&app.images_path.value);
    cfg.paths.vms       = PathBuf::from(&app.vms_path.value);
    cfg.paths.cloudinit = PathBuf::from(&app.cloudinit_path.value);
    cfg
}

