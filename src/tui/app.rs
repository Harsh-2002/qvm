//! Application state for the TUI. Pure logic, no terminal I/O.
//!
//! Layout shift from the v1 single-pane-plus-modals to a Proxmox-style
//! split pane: persistent sidebar listing all VMs, right pane showing the
//! selected VM's details (or an inline form / help screen / empty state).
//! Modals are gone; destructive confirms appear in the bottom status bar.

use crate::config::Config;
use crate::libvirt;
use crate::tui::events::Action;
use crate::tui::forms::TextInput;
use crate::tui::theme::Theme;
use crate::util;
use ratatui::layout::Rect;
use std::time::{Duration, Instant};

/// Which pane the keyboard is currently driving. Tab cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane { Sidebar, Detail }
impl FocusPane {
    pub fn next(self) -> Self {
        match self { FocusPane::Sidebar => FocusPane::Detail, FocusPane::Detail => FocusPane::Sidebar }
    }
}

const TICK: Duration = Duration::from_secs(2);
const TOAST_TTL: Duration = Duration::from_secs(5);

/// What the right-hand content pane is showing right now.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    /// Default — selected VM's metadata + dominfo.
    Detail,
    /// Inline form to create a new VM (replaces the detail pane).
    CreateForm,
    /// Bottom-bar confirm for delete (Detail still visible behind it).
    ConfirmDelete,
    /// Help screen listing keybindings.
    Help,
    /// Filter input active in the sidebar (Detail still visible).
    Filter,
    /// No VMs exist — welcome message + create hint.
    EmptyState,
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
    /// Cached `virsh dominfo` for the selected VM. Updated on tick.
    pub dominfo: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Toast { Ok(String), Err(String) }

#[derive(Debug)]
pub struct App {
    pub rows: Vec<VmRow>,
    pub selected: usize,
    /// Scroll offset for the dominfo region of the Detail pane.
    pub detail_scroll: u16,
    /// Toggle: show raw `virsh dominfo` instead of structured metadata.
    pub show_raw_dominfo: bool,
    pub mode: Mode,
    pub should_quit: bool,
    pub filter: String,
    pub filter_input: TextInput,
    pub sort: Sort,
    pub toast: Option<(Toast, Instant)>,
    pub last_refresh: Option<Instant>,
    pub create: CreateForm,
    pub host_label: String,
    pub theme: Theme,
    /// Animation tick — increments every render frame. Used for spinners.
    pub tick: u64,
    /// Which pane has keyboard focus (toggled by Tab).
    pub focused: FocusPane,
    /// True while a `libvirt::domains()` refresh is in flight (shows spinner).
    pub is_refreshing: bool,
    /// Optimistic state shown next to a row until the next refresh resolves it.
    /// `(vm_name, displayed_state)` — e.g. `("web01", "starting…")`.
    pub pending: Option<(String, &'static str)>,
    /// Mouse hit-test targets — rebuilt every render.
    /// `(area, visible_index)` for sidebar VM rows.
    pub sidebar_hits: Vec<(Rect, usize)>,
    /// `(area, hotkey_char)` for action-bar buttons.
    pub action_hits: Vec<(Rect, char)>,
}

#[derive(Debug, Clone, Default)]
pub struct CreateForm {
    pub field:       usize, // 0..=6
    pub name:        TextInput,
    pub distro_idx:  usize, // index into available distros
    pub distros:     Vec<String>,
    pub distro_pulled: Vec<bool>,
    pub cpus:        TextInput,
    pub memory_gb:   TextInput,
    pub disk_gb:     TextInput,
    pub user:        TextInput,
    pub password:    TextInput,
}

impl App {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            detail_scroll: 0,
            show_raw_dominfo: false,
            mode: Mode::EmptyState,
            should_quit: false,
            filter: String::new(),
            filter_input: TextInput::default(),
            sort: Sort::Name,
            toast: None,
            last_refresh: None,
            create: CreateForm::default(),
            host_label: detect_host_label(),
            theme: Theme::default(),
            tick: 0,
            focused: FocusPane::Sidebar,
            is_refreshing: false,
            pending: None,
            sidebar_hits: Vec::new(),
            action_hits: Vec::new(),
        }
    }

    pub fn tick_due(&self) -> bool {
        match self.last_refresh {
            Some(t) => t.elapsed() >= TICK,
            None    => true,
        }
    }

    /// Refresh from libvirt. Best-effort: surfaces failures as a toast.
    ///
    /// IP lookups are CACHED across refreshes — once we have an IP for a
    /// running VM, we don't re-query virsh agent every tick (each query
    /// shells out to virsh up to 3× via agent/lease/arp fallback chain).
    /// IPs only change on lease renewals, which are minutes apart at best;
    /// the cost of a stale IP for a few seconds is negligible compared to
    /// the latency of re-querying every tick.
    pub fn refresh(&mut self, _cfg: &Config) {
        self.is_refreshing = true;
        self.last_refresh = Some(Instant::now());
        let result = libvirt::domains();
        self.is_refreshing = false;
        match result {
            Ok(doms) => {
                let prev_selected_name = self.selected_name();
                // Reuse IPs we already learned for VMs that are still present.
                let prev_ips: std::collections::HashMap<String, String> =
                    self.rows.iter()
                        .filter_map(|r| r.ip.as_ref().map(|ip| (r.name.clone(), ip.clone())))
                        .collect();
                self.rows = doms.into_iter().map(|d| {
                    let ip = if d.state == "running" {
                        prev_ips.get(&d.name).cloned().or_else(|| libvirt::ipv4(&d.name))
                    } else {
                        None
                    };
                    VmRow { ip, name: d.name, state: d.state, dominfo: None }
                }).collect();
                self.apply_sort();

                // Restore selection by name if possible.
                if let Some(prev) = prev_selected_name {
                    if let Some(i) = self.visible().iter().position(|r| r.name == prev) {
                        self.selected = i;
                    }
                }
                let visible = self.visible_count();
                if visible == 0 {
                    self.selected = 0;
                    if !matches!(self.mode, Mode::CreateForm | Mode::Help) {
                        self.mode = Mode::EmptyState;
                    }
                } else {
                    if self.selected >= visible { self.selected = visible - 1; }
                    if matches!(self.mode, Mode::EmptyState) {
                        self.mode = Mode::Detail;
                    }
                }

                // Pull dominfo for the selected VM only.
                if let Some(name) = self.selected_name() {
                    if let Ok(s) = libvirt::dominfo(&name) {
                        if let Some(idx) = self.visible_index() {
                            if let Some(actual) = self.row_index_for_visible(idx) {
                                self.rows[actual].dominfo = Some(s);
                            }
                        }
                    }
                }
                // Clear pending optimistic state once the real state catches up.
                if let Some((pname, _)) = &self.pending {
                    if !self.rows.iter().any(|r| &r.name == pname) {
                        // VM gone (e.g. deletion confirmed) — clear.
                        self.pending = None;
                    } else {
                        // For lifecycle actions: clear after one refresh tick.
                        // The real state from virsh now drives the row.
                        self.pending = None;
                    }
                }
            }
            Err(e) => self.toast_err(format!("refresh failed: {e}")),
        }
    }

    /// What state should we display for a row, accounting for optimistic
    /// pending updates? Returns the row's actual state unless `pending`
    /// matches that row.
    pub fn displayed_state(&self, row: &VmRow) -> String {
        if let Some((n, s)) = &self.pending {
            if n == &row.name { return (*s).to_string(); }
        }
        row.state.clone()
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
    pub fn selected_row(&self) -> Option<&VmRow> {
        self.visible().get(self.selected).copied()
    }
    pub fn selected_state(&self) -> Option<String> {
        self.visible().get(self.selected).map(|r| r.state.clone())
    }
    fn visible_index(&self) -> Option<usize> {
        if self.selected < self.visible_count() { Some(self.selected) } else { None }
    }
    fn row_index_for_visible(&self, vis_idx: usize) -> Option<usize> {
        let name = self.visible().get(vis_idx).map(|r| r.name.clone())?;
        self.rows.iter().position(|r| r.name == name)
    }

    pub fn toast_ok(&mut self, m: String)  { self.toast = Some((Toast::Ok(m),  Instant::now())); }
    pub fn toast_err(&mut self, m: String) { self.toast = Some((Toast::Err(m), Instant::now())); }
    pub fn current_toast(&self) -> Option<&Toast> {
        self.toast.as_ref().and_then(|(t, when)|
            if when.elapsed() < TOAST_TTL { Some(t) } else { None })
    }

    pub fn apply(&mut self, action: Action, cfg: &Config) {
        match action {
            Action::Quit          => self.should_quit = true,
            Action::CycleFocus    => { self.focused = self.focused.next(); }
            Action::SelectIndex(i) => {
                if i < self.visible_count() { self.selected = i; self.detail_scroll = 0; }
            }
            Action::ToggleRaw     => { self.show_raw_dominfo = !self.show_raw_dominfo; self.detail_scroll = 0; }
            Action::Down          => { self.move_selection(1); self.detail_scroll = 0; }
            Action::Up            => { self.move_selection(-1); self.detail_scroll = 0; }
            Action::ScrollDetailDown => self.detail_scroll = self.detail_scroll.saturating_add(1),
            Action::ScrollDetailUp   => self.detail_scroll = self.detail_scroll.saturating_sub(1),
            Action::OpenCreate    => self.open_create(cfg),
            Action::OpenDelete    => {
                if self.selected_name().is_some() {
                    self.mode = Mode::ConfirmDelete;
                }
            }
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
            Action::ShowVnc       => self.show_vnc_toast(cfg),
            Action::CloseToDetail => self.close_to_detail(),
            Action::Start         => self.act_lifecycle("start",   libvirt::start),
            Action::Stop          => self.act_lifecycle("stop",    libvirt::shutdown),
            Action::Restart       => self.act_lifecycle("restart", libvirt::reboot),
            Action::ConfirmDelete => self.act_delete(cfg),
            // Create-form interactions.
            Action::CreateNext    => self.create.field = (self.create.field + 1) % 7,
            Action::CreatePrev    => self.create.field = (self.create.field + 6) % 7,
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
                self.close_to_detail();
                self.selected = 0;
            }
            Action::FilterCancel      => {
                self.filter_input.clear();
                self.close_to_detail();
            }
            // Actions handled in tui/mod.rs (suspend+exec) — should never reach here.
            Action::Console | Action::Browser | Action::SubmitCreate | Action::Noop => {}
        }
        if let Some((_, when)) = self.toast {
            if when.elapsed() > TOAST_TTL * 4 { self.toast = None; }
        }
    }

    fn close_to_detail(&mut self) {
        if self.visible_count() == 0 {
            self.mode = Mode::EmptyState;
        } else {
            self.mode = Mode::Detail;
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
        // Optimistic state — clears on next refresh.
        let pending_label: &'static str = match verb {
            "start"   => "starting…",
            "stop"    => "stopping…",
            "restart" => "restarting…",
            _         => "…",
        };
        self.pending = Some((name.clone(), pending_label));
        match f(&name) {
            Ok(()) => self.toast_ok(format!("{verb}: '{name}'")),
            Err(e) => { self.pending = None; self.toast_err(format!("{verb} '{name}' failed: {e}")); }
        }
    }

    fn act_delete(&mut self, cfg: &Config) {
        let name = match self.selected_name() { Some(n) => n, None => return };
        match crate::commands::delete::run(cfg, &name, /* force */ true) {
            Ok(()) => self.toast_ok(format!("Deleted '{name}'")),
            Err(e) => self.toast_err(format!("delete '{name}' failed: {e}")),
        }
        self.close_to_detail();
        self.refresh(cfg);
    }

    fn open_create(&mut self, cfg: &Config) {
        let distros: Vec<String> = cfg.distros.keys().cloned().collect();
        let distro_pulled: Vec<bool> = distros.iter()
            .map(|k| cfg.image_path(k).map(|p| p.exists()).unwrap_or(false))
            .collect();
        let mut f = CreateForm {
            distros,
            distro_pulled,
            cpus:      TextInput::with_value(cfg.defaults.cpus.to_string()),
            memory_gb: TextInput::with_value(cfg.defaults.memory_gb.to_string()),
            disk_gb:   TextInput::with_value(cfg.defaults.disk_gb.to_string()),
            ..Default::default()
        };
        if let Some(idx) = f.distros.iter().position(|d| d == &cfg.defaults.distro) {
            f.distro_idx = idx;
        }
        self.create = f;
        self.mode = Mode::CreateForm;
    }

    fn show_vnc_toast(&mut self, cfg: &Config) {
        let Some(name) = self.selected_name() else { return };
        if self.selected_state().as_deref() != Some("running") {
            self.toast_err(format!("'{name}' is not running"));
            return;
        }
        match libvirt::vnc_endpoint(&name) {
            Some(ep) => {
                let bind = &cfg.vnc.bind;
                self.toast_ok(format!(
                    "VNC: {bind}:{} (port {}) — open vnc://{bind}",
                    ep.display, ep.port,
                ));
            }
            None => self.toast_err(format!("'{name}' has no VNC display")),
        }
    }

    fn create_focused_mut<F: FnOnce(&mut TextInput)>(&mut self, f: F) {
        match self.create.field {
            0 => f(&mut self.create.name),
            // 1 (distro) is not a text field — left/right cycles it.
            2 => f(&mut self.create.cpus),
            3 => f(&mut self.create.memory_gb),
            4 => f(&mut self.create.disk_gb),
            5 => f(&mut self.create.user),
            6 => f(&mut self.create.password),
            _ => {}
        }
    }

    fn create_insert(&mut self, c: char) {
        match self.create.field {
            1 => {}
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
        let cpus      = parse_pos(&self.create.cpus, "CPUs")?;
        let memory_gb = parse_pos(&self.create.memory_gb, "RAM (GB)")?;
        let disk_gb   = parse_pos(&self.create.disk_gb, "Disk (GB)")?;
        let user = {
            let s = self.create.user.value.trim();
            if s.is_empty() {
                return Err("login user required (no default)".into());
            }
            s.to_string()
        };
        let password = {
            let s = self.create.password.value.trim();
            if s.is_empty() {
                return Err("password required (no default)".into());
            }
            s.to_string()
        };
        self.close_to_detail();
        Ok(crate::commands::create::Args {
            name,
            distro: Some(distro),
            cpus: Some(cpus),
            memory_gb: Some(memory_gb),
            disk_gb: Some(disk_gb),
            user: Some(user),
            password: Some(password),
            no_autostart: false,
        })
    }
}

impl Default for App { fn default() -> Self { Self::new() } }

fn detect_host_label() -> String {
    crate::cmd::run("hostname", std::iter::empty::<&str>())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this-host".into())
}
