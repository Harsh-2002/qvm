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
    /// Inline form to change CPU / RAM / disk on the selected VM.
    ResizeForm,
    /// Bottom-bar confirm for delete (Detail still visible behind it).
    ConfirmDelete,
    /// Help screen listing keybindings.
    Help,
    /// Filter input active in the sidebar (Detail still visible).
    Filter,
    /// No VMs exist — welcome message + create hint.
    EmptyState,
    /// Snapshot list for the selected VM (create/revert/delete inline).
    Snapshots,
}

/// Sub-state of the Snapshots view when the user has pressed a destructive
/// key and we're waiting for y/n confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotConfirm { Revert, Delete }

#[derive(Debug, Clone, Default)]
pub struct SnapshotsView {
    pub vm_name:  String,
    pub snaps:    Vec<String>, // newest first
    pub selected: usize,
    pub confirm:  Option<SnapshotConfirm>,
}

impl SnapshotsView {
    pub fn selected_snap(&self) -> Option<&str> {
        self.snaps.get(self.selected).map(|s| s.as_str())
    }
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
    /// Count of files on disk with no matching libvirt domain. Computed
    /// once at TUI startup. Non-zero values render a hint in the header
    /// pointing at `qvm cleanup`.
    pub orphan_count: usize,
    /// Inline resize form for the selected VM. Populated when the user
    /// presses [m]; consumed when they Enter to apply.
    pub resize: ResizeForm,
    /// Snapshot view state. Populated when the user presses [p].
    pub snapshots: SnapshotsView,
}

/// Inline resize form. Pre-populated with the selected VM's current
/// CPU / RAM / disk; the user changes any of the three.
///
/// CPU + RAM apply on next reboot (libvirt's --config flag — virtio
/// memory ballooning could shrink/grow live but qvm doesn't expose
/// that knob). Disk grow requires the VM to be stopped (qemu-img can
/// corrupt a live qcow2). Disk shrink is intentionally not supported
/// — it requires in-guest filesystem cooperation.
#[derive(Debug, Clone, Default)]
pub struct ResizeForm {
    pub field:     usize, // 0..=2
    pub vm_name:   String,
    pub cpus:      TextInput,
    pub memory_gb: TextInput,
    pub disk_gb:   TextInput,
    /// Originals for "did the user change this?" comparison.
    pub orig_cpus:      u32,
    pub orig_memory_gb: u32,
    pub orig_disk_gb:   u32,
}

#[derive(Debug, Clone, Default)]
pub struct CreateForm {
    pub field:       usize, // 0..=11
    pub name:        TextInput,
    pub distro_idx:  usize, // index into available distros
    pub distros:     Vec<String>,
    pub distro_pulled: Vec<bool>,
    pub cpus:        TextInput,
    pub memory_gb:   TextInput,
    pub disk_gb:     TextInput,
    pub user:        TextInput,
    pub password:    TextInput,
    /// Nested virtualization toggle — `true` means the new VM gets
    /// `--cpu host-passthrough` (can run KVM inside). Default true.
    pub nested:      bool,
    /// Run `package_update` + `package_upgrade` on first boot.
    /// Default false (off — long boot otherwise).
    pub upgrade:     bool,
    /// Persistent swap size (e.g. "1G", "512M"). Empty = no swap.
    pub swap:        TextInput,
    /// Static IPv4 CIDR (e.g. "10.1.1.50/24"). Empty = DHCP.
    pub ip:          TextInput,
    /// IPv4 default gateway. Required when `ip` is non-empty.
    pub gateway:     TextInput,
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
            orphan_count: 0,
            resize: ResizeForm::default(),
            snapshots: SnapshotsView::default(),
        }
    }

    pub fn tick_due(&self) -> bool {
        match self.last_refresh {
            Some(t) => t.elapsed() >= TICK,
            None    => true,
        }
    }

    /// Synchronous refresh — blocking on libvirt. Use this only for the
    /// initial pre-loop fetch and for post-action refreshes (after
    /// create/delete) where the user just performed an action and expects
    /// to see the result immediately. The 2-second tick refresh runs on a
    /// background thread; see [`apply_async_refresh`].
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
        let doms = match result {
            Ok(d) => d,
            Err(e) => { self.toast_err(format!("refresh failed: {e}")); return; }
        };
        // Reuse IPs we already learned for VMs that are still present.
        let prev_ips: std::collections::HashMap<String, String> =
            self.rows.iter()
                .filter_map(|r| r.ip.as_ref().map(|ip| (r.name.clone(), ip.clone())))
                .collect();
        let rows: Vec<VmRow> = doms.into_iter().map(|d| {
            let ip = if d.state == "running" {
                prev_ips.get(&d.name).cloned().or_else(|| libvirt::ipv4(&d.name))
            } else {
                None
            };
            VmRow { ip, name: d.name, state: d.state, dominfo: None }
        }).collect();
        // dominfo for the selected VM is fetched synchronously here so the
        // detail pane refreshes on user-initiated actions without waiting
        // for the next worker tick.
        let dominfo = self.selected_name()
            .and_then(|n| libvirt::dominfo(&n).ok().map(|raw| (n, raw)));
        self.apply_refresh(rows, dominfo);
    }

    /// Apply a refresh result produced by the background worker.
    /// See [`crate::tui::refresh`].
    pub fn apply_async_refresh(&mut self, result: crate::tui::refresh::RefreshResult) {
        self.last_refresh = Some(Instant::now());
        self.is_refreshing = false;
        self.apply_refresh(result.rows, result.selected_dominfo);
    }

    /// Common state transition between sync and async refresh. Pure logic.
    fn apply_refresh(&mut self, rows: Vec<VmRow>, selected_dominfo: Option<(String, String)>) {
        let prev_selected_name = self.selected_name();
        self.rows = rows;
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
            if !matches!(self.mode, Mode::CreateForm | Mode::ResizeForm | Mode::Help | Mode::Snapshots) {
                self.mode = Mode::EmptyState;
            }
        } else {
            if self.selected >= visible { self.selected = visible - 1; }
            if matches!(self.mode, Mode::EmptyState) {
                self.mode = Mode::Detail;
            }
        }

        // Splice dominfo onto the matching row.
        if let Some((name, raw)) = selected_dominfo {
            if let Some(row) = self.rows.iter_mut().find(|r| r.name == name) {
                row.dominfo = Some(raw);
            }
        }

        // Clear pending optimistic state — the real state has caught up.
        self.pending = None;
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
            Action::ToggleRaw     => { self.show_raw_dominfo = !self.show_raw_dominfo; self.detail_scroll = 0; }
            Action::Down          => { self.move_selection(1); self.detail_scroll = 0; }
            Action::Up            => { self.move_selection(-1); self.detail_scroll = 0; }
            Action::ScrollDetailDown => self.detail_scroll = self.detail_scroll.saturating_add(1),
            Action::ScrollDetailUp   => self.detail_scroll = self.detail_scroll.saturating_sub(1),
            Action::OpenCreate    => self.open_create(cfg),
            Action::OpenResize    => {
                if self.selected_name().is_some() { self.open_resize(cfg); }
            }
            Action::OpenDelete    => {
                if self.selected_name().is_some() {
                    self.mode = Mode::ConfirmDelete;
                }
            }
            Action::OpenHelp      => self.mode = Mode::Help,
            Action::OpenSnapshots => self.open_snapshots(),
            Action::SnapshotsUp   => self.snapshot_move(-1),
            Action::SnapshotsDown => self.snapshot_move(1),
            Action::SnapshotsRevertConfirm => {
                if self.snapshots.selected_snap().is_some() {
                    self.snapshots.confirm = Some(SnapshotConfirm::Revert);
                }
            }
            Action::SnapshotsDeleteConfirm => {
                if self.snapshots.selected_snap().is_some() {
                    self.snapshots.confirm = Some(SnapshotConfirm::Delete);
                }
            }
            Action::SnapshotsCancel => { self.snapshots.confirm = None; }
            // SnapshotsNew + SnapshotsConfirm are handled in tui/mod.rs because
            // they call virsh (libvirt::* via crate::commands::snap::*) and
            // then need to refresh the list — the refresh logic lives in App.
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
            Action::CreateNext    => self.create.field = (self.create.field + 1) % 12,
            Action::CreatePrev    => self.create.field = (self.create.field + 11) % 12,
            Action::CreateInsert(c) => self.create_insert(c),
            Action::CreateBackspace => self.create_focused_mut(|f| f.backspace()),
            Action::CreateDelete    => self.create_focused_mut(|f| f.delete()),
            Action::CreateLeft   => self.create_arrow_left(),
            Action::CreateRight  => self.create_arrow_right(),
            Action::CreateHome   => self.create_focused_mut(|f| f.home()),
            Action::CreateEnd    => self.create_focused_mut(|f| f.end()),
            // Resize-form interactions.
            Action::ResizeNext      => self.resize_next_field(),
            Action::ResizePrev      => self.resize_prev_field(),
            Action::ResizeInsert(c) => self.resize_insert(c),
            Action::ResizeBackspace => self.resize_backspace(),
            Action::ResizeDelete    => self.resize_delete(),
            Action::ResizeLeft      => self.resize_left(),
            Action::ResizeRight     => self.resize_right(),
            Action::ResizeHome      => self.resize_home(),
            Action::ResizeEnd       => self.resize_end(),
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
            // Actions handled in tui/mod.rs (suspend+exec or refresh required)
            // — should never reach here.
            Action::Console | Action::Browser
                | Action::SubmitCreate | Action::SubmitResize
                | Action::SnapshotsNew | Action::SnapshotsConfirm
                | Action::Noop => {}
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
            nested:    cfg.defaults.nested,
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
            0  => f(&mut self.create.name),
            // 1 (distro), 7 (nested), 8 (upgrade) are not text fields.
            2  => f(&mut self.create.cpus),
            3  => f(&mut self.create.memory_gb),
            4  => f(&mut self.create.disk_gb),
            5  => f(&mut self.create.user),
            6  => f(&mut self.create.password),
            9  => f(&mut self.create.swap),
            10 => f(&mut self.create.ip),
            11 => f(&mut self.create.gateway),
            _ => {}
        }
    }

    fn create_insert(&mut self, c: char) {
        match self.create.field {
            1 => {}
            2..=4 => {
                if c.is_ascii_digit() { self.create_focused_mut(|f| f.insert(c)); }
            }
            7 => {
                // Space toggles the nested-virt checkbox.
                if c == ' ' { self.create.nested = !self.create.nested; }
            }
            8 => {
                // Space toggles the upgrade-on-first-boot checkbox.
                if c == ' ' { self.create.upgrade = !self.create.upgrade; }
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
        } else if self.create.field == 7 {
            self.create.nested = !self.create.nested;
        } else if self.create.field == 8 {
            self.create.upgrade = !self.create.upgrade;
        } else { self.create_focused_mut(|f| f.left()); }
    }

    fn create_arrow_right(&mut self) {
        if self.create.field == 1 {
            if !self.create.distros.is_empty() {
                self.create.distro_idx = (self.create.distro_idx + 1) % self.create.distros.len();
            }
        } else if self.create.field == 7 {
            self.create.nested = !self.create.nested;
        } else if self.create.field == 8 {
            self.create.upgrade = !self.create.upgrade;
        } else { self.create_focused_mut(|f| f.right()); }
    }

    /// Open the snapshot list view for the currently selected VM.
    /// Populates the list synchronously (one virsh shell-out, fast).
    fn open_snapshots(&mut self) {
        let name = match self.selected_name() { Some(n) => n, None => return };
        let snaps = crate::commands::snap::list_parsed(&name).unwrap_or_default();
        self.snapshots = SnapshotsView {
            vm_name: name,
            snaps,
            selected: 0,
            confirm: None,
        };
        self.mode = Mode::Snapshots;
    }

    /// Reload the snapshot list (after create / revert / delete). Best-
    /// effort: if virsh errors, the existing list is preserved and the
    /// caller is responsible for surfacing the failure via toast.
    pub fn refresh_snapshots(&mut self) {
        if self.snapshots.vm_name.is_empty() { return; }
        if let Ok(v) = crate::commands::snap::list_parsed(&self.snapshots.vm_name) {
            let n = v.len();
            self.snapshots.snaps = v;
            if n == 0 { self.snapshots.selected = 0; }
            else if self.snapshots.selected >= n { self.snapshots.selected = n - 1; }
        }
    }

    fn snapshot_move(&mut self, delta: i32) {
        let n = self.snapshots.snaps.len();
        if n == 0 { return; }
        let i = self.snapshots.selected as i32 + delta;
        let i = if i < 0 { (n as i32 - 1).max(0) } else if i as usize >= n { 0 } else { i };
        self.snapshots.selected = i as usize;
    }

    /// Open the inline resize form for the selected VM. Best-effort:
    /// pre-fills from `virsh dominfo` + `qemu-img info`. Falls back to
    /// blank fields if either lookup fails (the user can still type).
    fn open_resize(&mut self, cfg: &Config) {
        let name = match self.selected_name() { Some(n) => n, None => return };
        let (cpus_now, mem_now) = current_cpus_mem(&name).unwrap_or((0, 0));
        let disk_now = current_disk_gb(cfg, &name).unwrap_or(0);
        self.resize = ResizeForm {
            field:     0,
            vm_name:   name,
            cpus:      TextInput::with_value(cpus_now.to_string()),
            memory_gb: TextInput::with_value(mem_now.to_string()),
            disk_gb:   TextInput::with_value(disk_now.to_string()),
            orig_cpus:      cpus_now,
            orig_memory_gb: mem_now,
            orig_disk_gb:   disk_now,
        };
        self.mode = Mode::ResizeForm;
    }

    fn resize_focused_mut<F>(&mut self, f: F) where F: FnOnce(&mut TextInput) {
        let r = &mut self.resize;
        match r.field {
            0 => f(&mut r.cpus),
            1 => f(&mut r.memory_gb),
            2 => f(&mut r.disk_gb),
            _ => {}
        }
    }

    pub fn resize_next_field(&mut self) { self.resize.field = (self.resize.field + 1) % 3; }
    pub fn resize_prev_field(&mut self) { self.resize.field = (self.resize.field + 2) % 3; }

    pub fn resize_insert(&mut self, c: char) {
        // Only digits — these are integer GB / count fields.
        if c.is_ascii_digit() {
            self.resize_focused_mut(|t| t.insert(c));
        }
    }
    pub fn resize_backspace(&mut self) { self.resize_focused_mut(|t| t.backspace()); }
    pub fn resize_delete(&mut self)    { self.resize_focused_mut(|t| t.delete()); }
    pub fn resize_left(&mut self)      { self.resize_focused_mut(|t| t.left()); }
    pub fn resize_right(&mut self)     { self.resize_focused_mut(|t| t.right()); }
    pub fn resize_home(&mut self)      { self.resize_focused_mut(|t| t.home()); }
    pub fn resize_end(&mut self)       { self.resize_focused_mut(|t| t.end()); }

    /// Pull out the resize plan: parsed CPUs / RAM / disk + the original
    /// values so the executor knows what actually changed. Closes the
    /// form on success.
    pub fn take_resize_args(&mut self) -> std::result::Result<ResizeArgs, String> {
        let parse_pos = |t: &TextInput, what: &str| -> std::result::Result<u32, String> {
            let s = t.value.trim();
            if s.is_empty() { return Err(format!("{what} required")); }
            let n: u32 = s.parse().map_err(|_| format!("{what} must be a number"))?;
            if n == 0 { return Err(format!("{what} must be > 0")); }
            Ok(n)
        };
        let cpus    = parse_pos(&self.resize.cpus, "CPUs")?;
        let mem_gb  = parse_pos(&self.resize.memory_gb, "RAM (GB)")?;
        let disk_gb = parse_pos(&self.resize.disk_gb, "Disk (GB)")?;
        // Disk shrink is refused — we don't have a safe path for it.
        if disk_gb < self.resize.orig_disk_gb {
            return Err(format!(
                "disk shrink ({} → {} GB) is not supported. Manual route: \
                 shrink the FS inside the guest, then `qemu-img resize --shrink`.",
                self.resize.orig_disk_gb, disk_gb
            ));
        }
        let name = self.resize.vm_name.clone();
        let plan = ResizeArgs {
            name,
            cpus,
            memory_gb: mem_gb,
            disk_gb,
            orig_cpus:      self.resize.orig_cpus,
            orig_memory_gb: self.resize.orig_memory_gb,
            orig_disk_gb:   self.resize.orig_disk_gb,
        };
        self.close_to_detail();
        Ok(plan)
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
        let nested = self.create.nested;
        self.close_to_detail();
        // Whitespace-only values mean "leave it unset" — the TUI fields
        // are free-text, blank rows shouldn't ever be coerced into
        // `Some("")` and tripped over by the CLI validation downstream.
        let maybe = |s: &str| {
            let t = s.trim();
            if t.is_empty() { None } else { Some(t.to_string()) }
        };
        let upgrade = self.create.upgrade;
        let swap    = maybe(&self.create.swap.value);
        let ip      = maybe(&self.create.ip.value);
        let gateway = maybe(&self.create.gateway.value);

        Ok(crate::commands::create::Args {
            name,
            distro: Some(distro),
            cpus: Some(cpus),
            memory_gb: Some(memory_gb),
            disk_gb: Some(disk_gb),
            user: Some(user),
            password: Some(password),
            no_autostart: false,
            nested: Some(nested),
            upgrade,
            swap,
            ip,
            gateway,
        })
    }
}

impl Default for App { fn default() -> Self { Self::new() } }

/// What the resize executor needs to apply the plan. Lives outside App
/// so the tui/mod.rs handler can take ownership while the App is
/// already in Mode::Detail.
#[derive(Debug, Clone)]
pub struct ResizeArgs {
    pub name: String,
    pub cpus: u32,
    pub memory_gb: u32,
    pub disk_gb: u32,
    pub orig_cpus: u32,
    pub orig_memory_gb: u32,
    pub orig_disk_gb: u32,
}

/// Parse libvirt `virsh dominfo <name>` for (CPUs, memory_gb).
fn current_cpus_mem(name: &str) -> Option<(u32, u32)> {
    let raw = crate::libvirt::dominfo(name).ok()?;
    let mut cpus = None;
    let mut mem_gb = None;
    for line in raw.lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("CPU(s):") {
            cpus = v.trim().parse().ok();
        } else if let Some(rest) = t.strip_prefix("Max memory:") {
            let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
            mem_gb = Some(kib.div_ceil(1024 * 1024) as u32);
        }
    }
    Some((cpus?, mem_gb?))
}

/// Inspect the on-disk qcow2 for its virtual size (GB, rounded up).
fn current_disk_gb(cfg: &Config, name: &str) -> Option<u32> {
    let disk = cfg.vm_disk(name);
    let out = crate::cmd::run("qemu-img", [
        "info", "--output=human", disk.to_string_lossy().as_ref(),
    ]).ok()?;
    for line in out.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("virtual size:") {
            // "virtual size: 50 GiB (...)" — take the GiB count, round up.
            if let Some(gib_idx) = rest.find(" GiB") {
                let n_str: String = rest[..gib_idx].chars()
                    .filter(|c| c.is_ascii_digit() || *c == '.').collect();
                if let Ok(n) = n_str.trim().parse::<f64>() {
                    return Some(n.ceil() as u32);
                }
            }
            // Fallback: "N bytes (...)" form.
            let n_str: String = rest.split_whitespace().next()?.chars()
                .filter(|c| c.is_ascii_digit()).collect();
            if let Ok(bytes) = n_str.parse::<u64>() {
                return Some(bytes.div_ceil(1024u64.pow(3)) as u32);
            }
        }
    }
    None
}

fn detect_host_label() -> String {
    crate::cmd::run("hostname", std::iter::empty::<&str>())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this-host".into())
}
