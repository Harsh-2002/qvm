//! Background refresh worker for the TUI.
//!
//! The main render loop must not block on `virsh` shell-outs — with N VMs
//! the every-2-second refresh would freeze the spinner and starve key
//! events. Instead, a dedicated thread loops:
//!
//!   ┌─ send `Starting` ─────────────── main loop flips spinner on
//!   │
//!   ├─ libvirt::domains()
//!   ├─ libvirt::ipv4(<each running>) (cache hits reuse old IPs)
//!   ├─ libvirt::dominfo(<selected>)  (one extra shell-out per tick)
//!   │
//!   ├─ send `Result { … }` ─────────── main loop applies + spinner off
//!   │
//!   └─ sleep 2 s, loop
//!
//! The selected VM name is shared via an `Arc<Mutex<Option<String>>>` so
//! the worker knows which row's dominfo to refresh.

use crate::config::Config;
use crate::libvirt::{self, Domain};
use crate::tui::app::VmRow;
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Cadence between refreshes. Matches the previous synchronous 2-second tick.
pub const TICK: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum RefreshMsg {
    /// Worker has started a new refresh cycle. Main loop flips the spinner on.
    Starting,
    /// Worker finished a refresh; apply this and flip the spinner off.
    Result(RefreshResult),
}

#[derive(Debug, Default)]
pub struct RefreshResult {
    pub rows: Vec<VmRow>,
    /// dominfo for whichever VM was selected when the worker started this
    /// iteration. `(name, raw)`; the apply step matches `name` against
    /// `rows` and stores the string on the matching row.
    pub selected_dominfo: Option<(String, String)>,
}

/// Spawn the refresh worker. Returns a JoinHandle but the caller usually
/// doesn't need to wait on it — when the receiver is dropped (TUI exiting),
/// the worker's next send fails and it exits naturally.
pub fn spawn(
    cfg: Config,
    selected: Arc<Mutex<Option<String>>>,
    tx: Sender<RefreshMsg>,
) -> JoinHandle<()> {
    thread::spawn(move || worker_loop(cfg, selected, tx))
}

fn worker_loop(
    _cfg: Config,
    selected: Arc<Mutex<Option<String>>>,
    tx: Sender<RefreshMsg>,
) {
    // Per-worker IP cache. Same shape as the inline refresh used: once we
    // resolve an IP for a running VM we hang on to it until the VM stops.
    let mut ip_cache: HashMap<String, String> = HashMap::new();

    loop {
        if tx.send(RefreshMsg::Starting).is_err() { return; }

        let domains: Vec<Domain> = libvirt::domains().unwrap_or_default();
        // Drop cached IPs for VMs that no longer exist or aren't running.
        let live_running: std::collections::HashSet<&str> = domains.iter()
            .filter(|d| d.state == "running")
            .map(|d| d.name.as_str())
            .collect();
        ip_cache.retain(|k, _| live_running.contains(k.as_str()));

        let rows: Vec<VmRow> = domains.into_iter().map(|d| {
            let ip = if d.state == "running" {
                if let Some(cached) = ip_cache.get(&d.name) {
                    Some(cached.clone())
                } else if let Some(fresh) = libvirt::ipv4(&d.name) {
                    ip_cache.insert(d.name.clone(), fresh.clone());
                    Some(fresh)
                } else {
                    None
                }
            } else {
                None
            };
            VmRow { ip, name: d.name, state: d.state, dominfo: None }
        }).collect();

        let selected_name: Option<String> = selected.lock().ok()
            .and_then(|g| g.clone());
        let selected_dominfo = selected_name.and_then(|n| {
            libvirt::dominfo(&n).ok().map(|raw| (n, raw))
        });

        if tx.send(RefreshMsg::Result(RefreshResult { rows, selected_dominfo })).is_err() {
            return;
        }
        thread::sleep(TICK);
    }
}
