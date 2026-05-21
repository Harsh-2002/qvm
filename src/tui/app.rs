//! Application state for the TUI. Pure logic, no terminal I/O.
//!
//! All actions that don't need raw-mode suspension (lifecycle, delete,
//! navigation, mode transitions, filter, sort) are applied here. The actions
//! that DO need to suspend the TUI (create, console) are handled in `mod.rs`,
//! which is the only file with terminal-state access.

use crate::config::Config;
use crate::libvirt;
use crate::tui::events::Action;
use crate::tui::forms::TextInput;
use crate::util;
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_secs(2);
const TOAST_TTL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Table,
    CreateForm,
    DeleteConfirm,
    Inspect { content: String, scroll: u16 },
    Vnc     { content: String },
    Help,
    Filter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort { Name, State, Ip }

impl Sort {
    fn next(self) -> Self {
        match self { Sort::Name => Sort::State, Sort::State => Sort::Ip, Sort::Ip => Sort::Name }
    }
    pub fn label(self) -> &'static str {
        match self { Sort::Name => "name", Sort::State => "state", Sort::Ip => "ip" }
    }
}

#[derive(Debug, Clone)]
pub struct VmRow {
    pub name:  String,
    pub state: String,
    pub ip:    Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Toast { Ok(String), Err(String) }

#[derive(Debug)]
pub struct App {
    pub rows: Vec<VmRow>,
    pub selected: usize,
    pub mode: Mode,
    pub should_quit: bool,
    pub filter: String,
    pub filter_input: TextInput,
    pub sort: Sort,
    pub toast: Option<(Toast, Instant)>,
    pub last_refresh: Option<Instant>,
    pub create: CreateForm,
}

#[derive(Debug, Clone, Default)]
pub struct CreateForm {
    pub field:       usize, // 0..=5
    pub name:        TextInput,
    pub distro_idx:  usize, // index into available distros
    pub distros:     Vec<String>,
    pub cpus:        TextInput,
    pub memory_gb:   TextInput,
    pub disk_gb:     TextInput,
    pub user:        TextInput,
}

impl App {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            mode: Mode::Table,
            should_quit: false,
            filter: String::new(),
            filter_input: TextInput::default(),
            sort: Sort::Name,
            toast: None,
            last_refresh: None,
            create: CreateForm::default(),
        }
    }

    pub fn tick_due(&self) -> bool {
        match self.last_refresh {
            Some(t) => t.elapsed() >= TICK,
            None    => true,
        }
    }

    /// Pull a fresh VM list from libvirt. Best-effort: a libvirt failure
    /// surfaces as a toast and leaves the previous rows visible.
    pub fn refresh(&mut self, _cfg: &Config) {
        self.last_refresh = Some(Instant::now());
        match libvirt::domains() {
            Ok(doms) => {
                self.rows = doms.into_iter().map(|d| VmRow {
                    ip: if d.state == "running" { libvirt::ipv4(&d.name) } else { None },
                    name: d.name,
                    state: d.state,
                }).collect();
                self.apply_sort();
                // Clamp selection.
                let visible = self.visible_count();
                if visible == 0 { self.selected = 0; }
                else if self.selected >= visible { self.selected = visible - 1; }
            }
            Err(e) => self.toast_err(format!("refresh failed: {e}")),
        }
    }

    fn apply_sort(&mut self) {
        let key = |r: &VmRow| -> String {
            match self.sort {
                Sort::Name  => r.name.clone(),
                Sort::State => format!("{}{}", r.state, r.name),
                Sort::Ip    => format!("{}{}", r.ip.clone().unwrap_or_default(), r.name),
            }
        };
        self.rows.sort_by_key(key);
    }

    /// Rows passing the filter.
    pub fn visible(&self) -> Vec<&VmRow> {
        if self.filter.is_empty() { self.rows.iter().collect() }
        else {
            let f = self.filter.to_ascii_lowercase();
            self.rows.iter()
                .filter(|r| r.name.to_ascii_lowercase().contains(&f))
                .collect()
        }
    }

    pub fn visible_count(&self) -> usize { self.visible().len() }

    pub fn selected_name(&self) -> Option<String> {
        self.visible().get(self.selected).map(|r| r.name.clone())
    }

    pub fn selected_state(&self) -> Option<String> {
        self.visible().get(self.selected).map(|r| r.state.clone())
    }

    pub fn toast_ok(&mut self, m: String)  { self.toast = Some((Toast::Ok(m),  Instant::now())); }
    pub fn toast_err(&mut self, m: String) { self.toast = Some((Toast::Err(m), Instant::now())); }

    pub fn current_toast(&self) -> Option<&Toast> {
        self.toast.as_ref().and_then(|(t, when)|
            if when.elapsed() < TOAST_TTL { Some(t) } else { None })
    }

    /// Apply a pure-state action. Actions that need raw-mode handling
    /// (Console, SubmitCreate) are dispatched in `tui/mod.rs`.
    pub fn apply(&mut self, action: Action, cfg: &Config) {
        match action {
            Action::Quit          => self.should_quit = true,
            Action::Down          => self.move_selection(1),
            Action::Up            => self.move_selection(-1),
            Action::OpenCreate    => self.open_create(cfg),
            Action::OpenDelete    => {
                if self.selected_name().is_some() { self.mode = Mode::DeleteConfirm; }
            }
            Action::OpenVnc       => self.open_vnc_popup(cfg),
            Action::OpenInspect   => self.open_inspect(),
            Action::OpenHelp      => self.mode = Mode::Help,
            Action::OpenFilter    => {
                self.filter_input = TextInput::with_value(&self.filter);
                self.mode = Mode::Filter;
            }
            Action::CycleSort     => {
                self.sort = self.sort.next();
                self.apply_sort();
                self.toast_ok(format!("Sorted by {}", self.sort.label()));
            }
            Action::CloseModal    => self.mode = Mode::Table,
            Action::Start         => self.act_lifecycle("start", libvirt::start),
            Action::Stop          => self.act_lifecycle("stop", libvirt::shutdown),
            Action::Restart       => self.act_lifecycle("restart", libvirt::reboot),
            Action::ConfirmDelete => self.act_delete(cfg),
            // Create-form interactions.
            Action::CreateNext    => self.create.field = (self.create.field + 1) % 6,
            Action::CreatePrev    => self.create.field = (self.create.field + 5) % 6,
            Action::CreateInsert(c) => self.create_insert(c),
            Action::CreateBackspace => self.create_focused_mut(|f| f.backspace()),
            Action::CreateDelete    => self.create_focused_mut(|f| f.delete()),
            Action::CreateLeft   => self.create_arrow_left(),
            Action::CreateRight  => self.create_arrow_right(),
            Action::CreateHome   => self.create_focused_mut(|f| f.home()),
            Action::CreateEnd    => self.create_focused_mut(|f| f.end()),
            // Filter interactions.
            Action::FilterInsert(c)   => self.filter_input.insert(c),
            Action::FilterBackspace   => self.filter_input.backspace(),
            Action::FilterDelete      => self.filter_input.delete(),
            Action::FilterLeft        => self.filter_input.left(),
            Action::FilterRight       => self.filter_input.right(),
            Action::FilterCommit      => {
                self.filter = self.filter_input.value.clone();
                self.mode = Mode::Table;
                self.selected = 0;
            }
            Action::FilterCancel      => self.mode = Mode::Table,
            // Inspect popup scrolling.
            Action::InspectScroll(d)  => self.inspect_scroll(d),
            // Actions handled in tui/mod.rs — should never reach here.
            Action::Console | Action::SubmitCreate | Action::Noop => {}
        }
        // Toast expiry is handled implicitly by `current_toast` filtering on
        // age, but we can drop the field once it's stale to save memory.
        if let Some((_, when)) = self.toast {
            if when.elapsed() > TOAST_TTL * 4 { self.toast = None; }
        }
    }

    fn move_selection(&mut self, delta: i32) {
        let n = self.visible_count();
        if n == 0 { return; }
        let i = self.selected as i32 + delta;
        let i = if i < 0 { (n as i32 - 1).max(0) } else if i as usize >= n { 0 } else { i };
        self.selected = i as usize;
    }

    fn act_lifecycle(&mut self, verb: &str, f: fn(&str) -> crate::error::Result<()>) {
        let name = match self.selected_name() { Some(n) => n, None => return };
        match f(&name) {
            Ok(()) => self.toast_ok(format!("{verb}: '{name}'")),
            Err(e) => self.toast_err(format!("{verb} '{name}' failed: {e}")),
        }
    }

    fn act_delete(&mut self, cfg: &Config) {
        let name = match self.selected_name() { Some(n) => n, None => return };
        match crate::commands::delete::run(cfg, &name, /* force */ true) {
            Ok(()) => self.toast_ok(format!("Deleted '{name}'")),
            Err(e) => self.toast_err(format!("delete '{name}' failed: {e}")),
        }
        self.mode = Mode::Table;
        self.refresh(cfg);
    }

    fn open_create(&mut self, cfg: &Config) {
        let mut f = CreateForm {
            distros: cfg.distros.keys().cloned().collect(),
            cpus:        TextInput::with_value(cfg.defaults.cpus.to_string()),
            memory_gb:   TextInput::with_value(cfg.defaults.memory_gb.to_string()),
            disk_gb:     TextInput::with_value(cfg.defaults.disk_gb.to_string()),
            ..Default::default()
        };
        if let Some(idx) = f.distros.iter().position(|d| d == &cfg.defaults.distro) {
            f.distro_idx = idx;
        }
        self.create = f;
        self.mode = Mode::CreateForm;
    }

    fn open_vnc_popup(&mut self, cfg: &Config) {
        let Some(name) = self.selected_name() else { return };
        if self.selected_state().as_deref() != Some("running") {
            self.toast_err(format!("'{name}' is not running"));
            return;
        }
        match libvirt::vnc_endpoint(&name) {
            Some(ep) => {
                let bind = &cfg.vnc.bind;
                let content = format!(
                    "VNC for '{name}'\n\n\
                     bind     {bind}\n\
                     display  :{}\n\
                     port     {}\n\n\
                     vncviewer {bind}:{}\n\
                     vncviewer {bind}::{}\n\n\
                     open vnc://{bind}",
                    ep.display, ep.port, ep.display, ep.port,
                );
                self.mode = Mode::Vnc { content };
            }
            None => self.toast_err(format!("'{name}' has no VNC display")),
        }
    }

    fn open_inspect(&mut self) {
        let Some(name) = self.selected_name() else { return };
        match libvirt::dominfo(&name) {
            Ok(content) => self.mode = Mode::Inspect { content, scroll: 0 },
            Err(e)      => self.toast_err(format!("inspect '{name}' failed: {e}")),
        }
    }

    fn inspect_scroll(&mut self, delta: i32) {
        if let Mode::Inspect { scroll, content } = &mut self.mode {
            let max = content.lines().count() as i32;
            let new = (*scroll as i32 + delta).clamp(0, (max - 1).max(0));
            *scroll = new as u16;
        }
    }

    fn create_focused_mut<F: FnOnce(&mut TextInput)>(&mut self, f: F) {
        match self.create.field {
            0 => f(&mut self.create.name),
            // 1 (distro) is not a text field — left/right cycles it via CreateLeft/Right.
            2 => f(&mut self.create.cpus),
            3 => f(&mut self.create.memory_gb),
            4 => f(&mut self.create.disk_gb),
            5 => f(&mut self.create.user),
            _ => {}
        }
    }

    fn create_insert(&mut self, c: char) {
        match self.create.field {
            1 => {} // distro field — ignore typing; use ←/→
            2..=4 => {
                if c.is_ascii_digit() { self.create_focused_mut(|f| f.insert(c)); }
            }
            _ => self.create_focused_mut(|f| f.insert(c)),
        }
    }

    fn create_arrow_left(&mut self) {
        if self.create.field == 1 {
            if !self.create.distros.is_empty() {
                let n = self.create.distros.len();
                self.create.distro_idx = (self.create.distro_idx + n - 1) % n;
            }
        } else { self.create_focused_mut(|f| f.left()); }
    }

    fn create_arrow_right(&mut self) {
        if self.create.field == 1 {
            if !self.create.distros.is_empty() {
                self.create.distro_idx = (self.create.distro_idx + 1) % self.create.distros.len();
            }
        } else { self.create_focused_mut(|f| f.right()); }
    }

    /// Build the `commands::create::Args` from the form, validating fields.
    /// Returns Err with a user-facing message on validation failure.
    pub fn take_create_args(&mut self) -> std::result::Result<crate::commands::create::Args, String> {
        let name = self.create.name.value.trim().to_string();
        if !util::valid_vm_name(&name) {
            return Err("invalid VM name (use letters, digits, . - _)".into());
        }
        let distro = self.create.distros.get(self.create.distro_idx)
            .cloned()
            .ok_or_else(|| "no distro selected".to_string())?;
        let parse_pos = |t: &TextInput, what: &str| -> std::result::Result<u32, String> {
            let s = t.value.trim();
            if s.is_empty() { return Err(format!("{what} required")); }
            let n: u32 = s.parse().map_err(|_| format!("{what} must be a number"))?;
            if n == 0 { return Err(format!("{what} must be > 0")); }
            Ok(n)
        };
        let cpus      = parse_pos(&self.create.cpus, "vCPUs")?;
        let memory_gb = parse_pos(&self.create.memory_gb, "RAM (GB)")?;
        let disk_gb   = parse_pos(&self.create.disk_gb, "Disk (GB)")?;
        let user = {
            let s = self.create.user.value.trim();
            if s.is_empty() { None } else { Some(s.to_string()) }
        };
        self.mode = Mode::Table;
        Ok(crate::commands::create::Args {
            name,
            distro: Some(distro),
            cpus: Some(cpus),
            memory_gb: Some(memory_gb),
            disk_gb: Some(disk_gb),
            user,
            password: None,
            no_autostart: false,
        })
    }
}

impl Default for App { fn default() -> Self { Self::new() } }
