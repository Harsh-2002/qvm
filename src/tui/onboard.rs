//! Step-by-step onboarding wizard.
//!
//! Triggered when `qvm` is launched with no subcommand AND
//! `/etc/qvm/config.yml` does not exist. Replaces the old
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
    "Welcome", "Host check", "Network", "DNS", "SSH keys",
    "Storage", "First image", "Done",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepKind {
    Welcome     = 0,
    HostCheck   = 1,
    Network     = 2,
    Dns         = 3,
    SshKeys     = 4,
    Paths       = 5,
    FirstImage  = 6,
    Done        = 7,
}

impl StepKind {
    fn next(self) -> Self {
        use StepKind::*;
        match self {
            Welcome    => HostCheck,
            HostCheck  => Network,
            Network    => Dns,
            Dns        => SshKeys,
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
            Dns        => Network,
            SshKeys    => Dns,
            Paths      => SshKeys,
            FirstImage => Paths,
            Done       => FirstImage,
        }
    }
    fn idx(self) -> usize { self as usize }
}

/// Which half of the SSH-keys step the keyboard drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SshFocus { Detected, Input }
impl SshFocus {
    fn toggle(self) -> Self {
        match self {
            SshFocus::Detected => SshFocus::Input,
            SshFocus::Input    => SshFocus::Detected,
        }
    }
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
    /// Active input line on the SSH-keys step. Accepts every character
    /// including space (real SSH keys are `ssh-X <body> <comment>`).
    /// Enter commits this to `added_keys` and clears it.
    paste_buf:     TextInput,
    /// Pasted keys the user has committed via Enter (separate from
    /// detected_keys). Listed below the input box.
    added_keys:    Vec<String>,
    /// Which section of the SSH-keys step has keyboard focus.
    ssh_focus:     SshFocus,
    /// Row cursor in the detected-keys list when ssh_focus == Detected.
    ssh_detected_idx: usize,

    /// DNS resolvers, comma-separated text the operator can edit. Parsed
    /// into a Vec<String> at the Done step.
    dns_input:      TextInput,

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
            added_keys:    Vec::new(),
            // Default focus to Input — most users have at most one detected
            // key and just want to start typing/pasting.
            ssh_focus:     SshFocus::Input,
            ssh_detected_idx: 0,

            // Pre-fill DNS with Cloudflare + Google so the common case is
            // "press Enter to accept". Blank → no DNS override (DHCP wins
            // for non-static VMs; static VMs fall back to 1.1.1.1/8.8.8.8
            // at create time anyway, per `create::run` logic).
            dns_input:      TextInput::with_value("1.1.1.1, 8.8.8.8"),

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

    /// Parse `dns_input` ("1.1.1.1, 8.8.8.8") into a Vec<String>.
    /// Tolerant of commas + spaces; empty entries dropped silently.
    fn dns(&self) -> Vec<String> {
        self.dns_input.value
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter_map(|s| {
                let t = s.trim();
                if t.is_empty() { None } else { Some(t.to_string()) }
            })
            .collect()
    }

    /// All SSH keys that will be written to config: every detected key
    /// whose checkbox is on, plus every key the user committed via the
    /// input box (`added_keys`). The currently-typed `paste_buf` content
    /// is NOT included until the user presses Enter — pressing Enter on
    /// empty buffer is the "I'm done" signal.
    fn selected_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.detected_keys.iter().enumerate()
            .filter(|(i, _)| self.key_selected.get(*i).copied().unwrap_or(false))
            .map(|(_, k)| k.clone())
            .collect();
        for extra in &self.added_keys {
            if !keys.contains(extra) {
                keys.push(extra.clone());
            }
        }
        keys
    }
}

// ── entry point ──────────────────────────────────────────────────────────────

pub fn run(cfg_path: &Path) -> Result<()> {
    // Install the same panic hook as the main TUI so a crash never leaves
    // the terminal in raw mode. Mouse capture is never enabled (keyboard-
    // only by design) so there's no DisableMouseCapture call here.
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

// ── SSH-keys step helpers ────────────────────────────────────────────────────

fn handle_ssh_detected(app: &mut OnboardApp, k: KeyEvent) {
    match k.code {
        KeyCode::Up | KeyCode::Char('k') if !app.detected_keys.is_empty() => {
            if app.ssh_detected_idx == 0 {
                app.ssh_detected_idx = app.detected_keys.len() - 1;
            } else {
                app.ssh_detected_idx -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') if !app.detected_keys.is_empty() => {
            app.ssh_detected_idx = (app.ssh_detected_idx + 1) % app.detected_keys.len();
        }
        KeyCode::Char(' ') => {
            if let Some(slot) = app.key_selected.get_mut(app.ssh_detected_idx) {
                *slot = !*slot;
            }
        }
        // Pressing Enter on the detected list jumps focus to the input box.
        // The actual "advance step" behavior lives in the Input branch
        // because empty-Enter is the universal "I'm done" gesture.
        KeyCode::Enter => {
            app.ssh_focus = SshFocus::Input;
        }
        _ => {}
    }
}

fn handle_ssh_input(app: &mut OnboardApp, k: KeyEvent) {
    match k.code {
        KeyCode::Left      => app.paste_buf.left(),
        KeyCode::Right     => app.paste_buf.right(),
        KeyCode::Home      => app.paste_buf.home(),
        KeyCode::End       => app.paste_buf.end(),
        KeyCode::Backspace => app.paste_buf.backspace(),
        KeyCode::Delete    => app.paste_buf.delete(),
        // Space is a real character here — SSH keys contain spaces
        // between the key-type, the body, and the comment.
        KeyCode::Char(c)   => app.paste_buf.insert(c),
        KeyCode::Enter => {
            let candidate = app.paste_buf.value.trim().to_string();
            if candidate.is_empty() {
                // Empty Enter = done. Apply the no-keys guard.
                if app.selected_keys().is_empty() {
                    app.flash = Some((
                        "No SSH keys selected. You won't be able to ssh in. Press [y] to continue anyway.".into(),
                        false,
                    ));
                } else {
                    app.step = app.step.next();
                    app.flash = None;
                }
            } else if !is_plausible_ssh_key(&candidate) {
                app.flash = Some((
                    "That doesn't look like an OpenSSH public key (expected ssh-ed25519 / ssh-rsa / ecdsa-sha2-... followed by a base64 body).".into(),
                    false,
                ));
            } else {
                // Commit and clear. Dedupe against both detected + already-added.
                let dup_detected = app.detected_keys.iter().any(|d| d == &candidate);
                let dup_added    = app.added_keys.iter().any(|d| d == &candidate);
                if dup_detected || dup_added {
                    app.flash = Some((
                        "Already in the list (skipped).".into(),
                        true,
                    ));
                } else {
                    app.added_keys.push(candidate);
                    app.flash = Some((
                        format!("Added (total: {}). Paste another or press Enter on empty to continue.",
                            app.added_keys.len()),
                        true,
                    ));
                }
                app.paste_buf = TextInput::default();
            }
        }
        _ => {}
    }
}

/// Cheap shape check: starts with an OpenSSH key-type prefix and has at
/// least three space-separated tokens (type, body, comment) — or two if
/// the user pasted without a comment.
/// Render an inline text input with a visible block-caret.
///
/// `focused` controls whether to draw the reverse-video caret. The
/// returned spans pre-truncate the value to `width` columns via
/// `TextInput::visible`, so long values stay editable without the
/// cursor disappearing offscreen.
///
/// This helper is the single place we paint inline cursors in the
/// onboarding flow; native terminal cursors aren't usable for inputs
/// rendered inside a multi-line Paragraph (which is what the wizard
/// uses).
fn caret_input<'a>(
    input:    &'a TextInput,
    width:    usize,
    focused:  bool,
    text_st:  Style,
    caret_st: Style,
) -> Vec<Span<'a>> {
    let (slice, cur_x) = input.visible(width.max(1));
    let chars: Vec<char> = slice.chars().collect();
    let left:  String = chars.iter().take(cur_x).collect();
    let right: String = chars.iter().skip(cur_x + 1).collect();
    let caret_ch: String = if cur_x < chars.len() {
        chars[cur_x].to_string()
    } else { " ".to_string() };
    let mut spans: Vec<Span<'a>> = Vec::with_capacity(3);
    if !left.is_empty() {
        spans.push(Span::styled(left, text_st));
    }
    if focused {
        spans.push(Span::styled(caret_ch, caret_st));
    } else if cur_x < chars.len() {
        spans.push(Span::styled(caret_ch, text_st));
    }
    if !right.is_empty() {
        spans.push(Span::styled(right, text_st));
    }
    spans
}

fn is_plausible_ssh_key(s: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "ssh-ed25519 ", "ssh-rsa ", "ssh-dss ",
        "ecdsa-sha2-nistp256 ", "ecdsa-sha2-nistp384 ", "ecdsa-sha2-nistp521 ",
        "sk-ssh-ed25519@openssh.com ", "sk-ecdsa-sha2-nistp256@openssh.com ",
    ];
    PREFIXES.iter().any(|p| s.starts_with(p)) && s.split_whitespace().count() >= 2
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
                let name = app.bridge();
                if name.is_empty() {
                    app.flash = Some(("Bridge name cannot be empty.".into(), false));
                } else if !bridge_exists(&name) {
                    // Soft warning: bridge name doesn't appear in /sys/class/net.
                    // Permit override on 'y' so installs from scripts (or
                    // pre-configured bridges that don't show until libvirt
                    // starts) still work.
                    app.flash = Some((
                        format!("Bridge '{name}' not found on this host. Press [y] to use it anyway."),
                        false,
                    ));
                } else {
                    app.step = app.step.next();
                    app.flash = None;
                }
            }
            KeyCode::Char('y') | KeyCode::Char('Y')
                if app.flash.is_some() && !app.bridge().is_empty() => {
                    app.step = app.step.next();
                    app.flash = None;
            }
            _ => {}
        },
        StepKind::Dns => match k.code {
            KeyCode::Char(c) => app.dns_input.insert(c),
            KeyCode::Backspace => app.dns_input.backspace(),
            KeyCode::Left      => app.dns_input.left(),
            KeyCode::Right     => app.dns_input.right(),
            KeyCode::Enter     => { app.step = app.step.next(); app.flash = None; }
            _ => {}
        },
        StepKind::SshKeys => {
            // Tab cycles focus between the detected-keys checklist and the
            // input box. Detected list only gets focus if there's at least
            // one detected key (otherwise Tab is a no-op).
            if k.code == KeyCode::Tab || k.code == KeyCode::BackTab {
                if !app.detected_keys.is_empty() {
                    app.ssh_focus = app.ssh_focus.toggle();
                }
                return Ok(());
            }
            // Dismiss "no keys" warning with y/Y. Runs before the focused-
            // input branch so a pending warning can be overridden even when
            // the input box has focus.
            if app.flash.is_some() && app.selected_keys().is_empty()
                && (k.code == KeyCode::Char('y') || k.code == KeyCode::Char('Y'))
            {
                app.step = app.step.next();
                app.flash = None;
                return Ok(());
            }
            match app.ssh_focus {
                SshFocus::Detected => handle_ssh_detected(app, k),
                SshFocus::Input    => handle_ssh_input(app, k),
            }
        }
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
            KeyCode::Enter     => {
                // Validate all three paths are writable. mkdir + touch +
                // unlink is the cheap end-to-end check: it catches
                // read-only mounts, missing permissions, and full disks.
                let dirs = [
                    ("images",    app.images_path.value.trim().to_string()),
                    ("vms",       app.vms_path.value.trim().to_string()),
                    ("cloudinit", app.cloudinit_path.value.trim().to_string()),
                ];
                let mut err: Option<String> = None;
                for (label, path) in &dirs {
                    if path.is_empty() {
                        err = Some(format!("{label} path cannot be empty."));
                        break;
                    }
                    if let Err(e) = check_writable(Path::new(path)) {
                        err = Some(format!("{label} path '{path}': {e}"));
                        break;
                    }
                }
                match err {
                    Some(e) => app.flash = Some((e, false)),
                    None    => { app.step = app.step.next(); app.flash = None; }
                }
            }
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
                    let distro = app.distro_choices[app.distro_sel].clone();
                    let cfg = build_partial_cfg(app);
                    // Quick HEAD-request to fail fast on DNS / firewall
                    // problems before downloading a gigabyte. Uses the
                    // same ureq client as pull::pull_one.
                    if let Ok(url) = cfg.image_url(&distro) {
                        app.flash = Some((format!("Checking {url} …"), true));
                        if let Err(e) = url_reachable(&url) {
                            app.flash = Some((
                                format!("{url} is unreachable: {e}. Press [Space] to skip pulling."),
                                false,
                            ));
                            return Ok(());
                        }
                    }
                    // Suspend ratatui so the streaming download's progress
                    // bar (from pull::pull_one) renders cleanly.
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
                let dns_list = app.dns();
                let toml = render_config(WizardAnswers {
                    bridge:         &app.bridge(),
                    dns:            &dns_list,
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
        StepKind::Dns        => draw_dns(f, inner, app),
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
    let text_st = if cur_custom {
        Style::default().fg(t.text).add_modifier(Modifier::BOLD)
    } else { Style::default().fg(t.text_dim) };
    let caret_st = Style::default().fg(t.accent).add_modifier(Modifier::REVERSED);
    // Bridge names are short (eth0, br0, virbr0) — 32 cols is plenty.
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(marker, Style::default().fg(t.accent)),
        Span::raw("  "),
        Span::styled("custom: ", Style::default().fg(t.text_dim)),
    ];
    spans.extend(caret_input(&app.bridge_custom, 32, cur_custom, text_st, caret_st));
    lines.push(Line::from(spans));
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        area.inner(Margin { horizontal: 2, vertical: 1 }),
    );
}

fn draw_dns(f: &mut Frame, area: Rect, app: &OnboardApp) {
    let t = &app.theme;
    let max_w: usize = (area.width as usize).saturating_sub(2 + 6 + 2);
    let text_st  = Style::default().fg(t.text).add_modifier(Modifier::BOLD);
    let caret_st = Style::default().fg(t.accent).add_modifier(Modifier::REVERSED);
    let mut input_spans = vec![
        Span::styled("  DNS:  ", Style::default().fg(t.text_dim)),
    ];
    // DNS step always has the input focused (it's the only thing on it).
    input_spans.extend(caret_input(&app.dns_input, max_w, true, text_st, caret_st));
    let lines: Vec<Line<'_>> = vec![
        Line::from(Span::styled(
            "DNS resolvers for new VMs.",
            Style::default().fg(t.text_dim))),
        Line::from(Span::styled(
            "Comma-separated list. Used only when a VM is given a static IP",
            Style::default().fg(t.text_faint))),
        Line::from(Span::styled(
            "(`qvm run --ip ...`); DHCP VMs ignore this and use the router's.",
            Style::default().fg(t.text_faint))),
        Line::raw(""),
        Line::from(input_spans),
        Line::raw(""),
        Line::from(Span::styled(
            "Blank line = no global DNS override (qvm falls back to 1.1.1.1 + 8.8.8.8",
            Style::default().fg(t.text_faint))),
        Line::from(Span::styled(
            "if you later create a static-IP VM without setting this).",
            Style::default().fg(t.text_faint))),
    ];
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
        Line::raw(""),
    ];

    // Detected list (with focus indicator).
    if app.detected_keys.is_empty() {
        lines.push(Line::from(Span::styled(
            "No keys found in ~/.ssh/ or /home/$SUDO_USER/.ssh/.",
            Style::default().fg(t.warn))));
        lines.push(Line::from(Span::styled(
            "Add one or more below.",
            Style::default().fg(t.text_dim))));
    } else {
        let header = if app.ssh_focus == SshFocus::Detected {
            "Detected (focus — [↑↓] choose, [Space] toggle, [Tab] to input):"
        } else {
            "Detected (press [Tab] to focus this list):"
        };
        lines.push(Line::from(Span::styled(header, Style::default().fg(t.text_dim))));
        for (i, k) in app.detected_keys.iter().enumerate() {
            let on = app.key_selected.get(i).copied().unwrap_or(false);
            let focused_row = app.ssh_focus == SshFocus::Detected && i == app.ssh_detected_idx;
            let cursor = if focused_row { "▸ " } else { "  " };
            let cursor_style = if focused_row {
                Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
            } else { Style::default().fg(t.text_faint) };
            let mark = if on { "[x]" } else { "[ ]" };
            let mark_style = if on {
                Style::default().fg(t.ok).add_modifier(Modifier::BOLD)
            } else { Style::default().fg(t.text_faint) };
            let preview: String = if k.chars().count() > 70 {
                k.chars().take(67).collect::<String>() + "..."
            } else { k.clone() };
            lines.push(Line::from(vec![
                Span::styled(cursor, cursor_style),
                Span::styled(mark, mark_style),
                Span::raw(" "),
                Span::styled(preview, Style::default().fg(t.text)),
            ]));
        }
    }
    lines.push(Line::raw(""));

    // Already-added list (committed via Enter).
    if !app.added_keys.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("Added ({}):", app.added_keys.len()),
            Style::default().fg(t.text_dim))));
        for k in &app.added_keys {
            let preview: String = if k.chars().count() > 70 {
                k.chars().take(67).collect::<String>() + "..."
            } else { k.clone() };
            lines.push(Line::from(vec![
                Span::styled("  • ", Style::default().fg(t.ok)),
                Span::styled(preview, Style::default().fg(t.text)),
            ]));
        }
        lines.push(Line::raw(""));
    }

    // Input box. Focus indicator + cursor mark.
    let input_label = if app.ssh_focus == SshFocus::Input {
        "Paste a key, press [Enter] to add it. Empty [Enter] when done:"
    } else {
        "Press [Tab] to focus the input box, then paste keys:"
    };
    lines.push(Line::from(Span::styled(input_label, Style::default().fg(t.text_dim))));
    let prompt_style = if app.ssh_focus == SshFocus::Input {
        Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
    } else { Style::default().fg(t.text_faint) };
    let preview_style = if app.paste_buf.value.is_empty() {
        Style::default().fg(t.text_faint)
    } else {
        Style::default().fg(t.text)
    };
    let focused_input = app.ssh_focus == SshFocus::Input;
    if app.paste_buf.value.is_empty() {
        // Empty: explicit block-caret on focus so the field doesn't look
        // dead. Without focus, show the muted "(empty)" placeholder.
        let placeholder = if focused_input { "(type or paste a key)" } else { "(empty)" };
        let mut spans = vec![Span::styled("  > ", prompt_style)];
        if focused_input {
            spans.push(Span::styled(" ", Style::default()
                .fg(t.accent).add_modifier(Modifier::REVERSED)));
        }
        spans.push(Span::styled(placeholder, if focused_input {
            Style::default().fg(t.text_faint)
        } else {
            preview_style
        }));
        lines.push(Line::from(spans));
    } else {
        // Non-empty: the shared helper handles scroll + inline caret.
        // SSH keys are ~80 chars on average so 80 cols is a sensible
        // window — the operator can still navigate the rest with arrow
        // keys, the window slides with the cursor.
        let caret_st = Style::default().fg(t.accent).add_modifier(Modifier::REVERSED);
        let mut spans = vec![Span::styled("  > ", prompt_style)];
        spans.extend(caret_input(&app.paste_buf, 80, focused_input, preview_style, caret_st));
        lines.push(Line::from(spans));
    }

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
    // 2 (left pad) + 1 (marker) + 1 (space) + 20 (label) + 2 (right margin)
    let input_width = (inner.width as usize).saturating_sub(2 + 1 + 1 + 20 + 2);
    let caret_st = Style::default().fg(t.accent).add_modifier(Modifier::REVERSED);
    let text_st  = Style::default().fg(t.text);
    for i in 0..3 {
        let focused = i == app.paths_field;
        let marker = if focused { "▸" } else { " " };
        let label_style = if focused {
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
        } else { Style::default().fg(t.text_dim) };
        let mut spans = vec![
            Span::raw("  "),
            Span::styled(marker, Style::default().fg(t.accent)),
            Span::raw(" "),
            Span::styled(format!("{:<20}", labels[i]), label_style),
        ];
        spans.extend(caret_input(inputs[i], input_width, focused, text_st, caret_st));
        lines.push(Line::from(spans));
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
            "On [Enter], the config will be written to /etc/qvm/config.yml and the main TUI will open.",
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
        StepKind::Dns        => "  [Enter] Continue   type DNS list, blank to skip   [Esc] Back",
        StepKind::SshKeys    => "  [Tab] Switch focus   [Enter] Add key / done   [Space] Toggle detected   [Esc] Back",
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

/// Does `/sys/class/net/<name>/bridge` exist? Used by the Network step
/// to warn the user when they enter a bridge name we can't see locally.
/// libvirt-managed bridges sometimes don't show in /sys until libvirtd
/// runs, so we treat this as a soft warning rather than a hard block.
fn bridge_exists(name: &str) -> bool {
    let p = format!("/sys/class/net/{name}/bridge");
    std::path::Path::new(&p).is_dir()
}

/// Create `path` if missing, then touch a probe file inside and remove it.
/// Returns the first OS error encountered. Pure side-effect test — leaves
/// `path` behind so `ensure_dirs` doesn't re-create it later.
fn check_writable(path: &std::path::Path) -> std::result::Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
    let probe = path.join(".qvm-write-test");
    std::fs::write(&probe, b"qvm").map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

/// Best-effort reachability check for a distro URL. Uses our embedded
/// HTTPS client (`ureq`) with a short timeout. Network problems (DNS,
/// firewall) surface here in ~2s instead of after a 1 GB partial download.
fn url_reachable(url: &str) -> std::result::Result<(), String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .new_agent();
    // HEAD is cheap; most cloud-image mirrors support it.
    match agent.head(url).call() {
        Ok(resp) => {
            let s = resp.status();
            if s.is_success() || s.is_redirection() {
                Ok(())
            } else {
                Err(format!("HTTP {s}"))
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

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

