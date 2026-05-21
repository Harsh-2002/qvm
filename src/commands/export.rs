//! `qvm export <vm> <out.qvm.tar>` — package a VM into a portable tarball.
//!
//! Two modes:
//!   - **Live**: when the VM is running and the guest agent responds, take
//!     a disk-only snapshot with `--quiesce`, convert the original on disk
//!     into the tarball, then `blockcommit --active --pivot` to merge the
//!     overlay back. Crash-consistent without stopping the VM (AWS-EBS-style).
//!   - **Offline**: stop the VM cleanly, convert, restart if it was running.
//!
//! `--live` forces live mode (fail if no guest agent).
//! `--stop`  forces offline mode (always stop + restart).
//! No flag  → auto-detect: prefer live when the VM is running and the agent
//!            is responsive; offline otherwise.

use crate::cmd;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Auto-detect: live if running + agent, otherwise offline.
    Auto,
    /// Force live; fail if the guest agent is not responsive.
    Live,
    /// Force offline; always stop the VM before exporting.
    Stop,
}

#[derive(Debug)]
pub struct Args {
    pub name: String,
    pub out:  PathBuf,
    pub mode: Mode,
    /// Optional retention dir: `--rotate-dir DIR --keep N` deletes oldest
    /// matching tarballs after a successful export.
    pub rotate_dir: Option<PathBuf>,
    pub keep:       Option<u32>,
}

pub fn run(cfg: &Config, a: Args) -> Result<()> {
    use crate::style as s;
    libvirt::require_defined(&a.name)?;
    cmd::require("qemu-img")?;
    cmd::require("tar")?;

    let was_running = libvirt::is_running(&a.name);

    let resolved_mode = match a.mode {
        Mode::Stop => ResolvedMode::Offline,
        Mode::Live => {
            if !was_running {
                return Err(Error::User(format!(
                    "--live requires '{}' to be running. Start it first or drop --live.",
                    a.name
                )));
            }
            require_guest_agent(&a.name)?;
            ResolvedMode::Live
        }
        Mode::Auto => {
            if was_running && guest_agent_alive(&a.name) {
                ResolvedMode::Live
            } else {
                ResolvedMode::Offline
            }
        }
    };

    let staging = mkstaging(&a.name)?;
    // best-effort cleanup of staging dir on any return path
    let _guard = StagingGuard(&staging);

    let disk_src   = cfg.vm_disk(&a.name);
    let staged_disk = staging.join("disk.qcow2");
    let staged_iso  = staging.join("cloud-init.iso");
    let staged_vmu  = staging.join(".vmuser");
    let staged_xml  = staging.join("domain.xml");
    let staged_meta = staging.join("qvm-meta.toml");

    // ── stage the disk (the long step) ────────────────────────────────────────
    let arch = host_arch();
    let cpus = dominfo_cpus(&a.name).unwrap_or(cfg.defaults.cpus);
    let mem_gb = dominfo_memory_gb(&a.name).unwrap_or(cfg.defaults.memory_gb);
    let disk_gb = qemu_img_virtual_size_gb(&disk_src).unwrap_or(cfg.defaults.disk_gb);

    match resolved_mode {
        ResolvedMode::Live => {
            println!("{} exporting '{}' (live, --quiesce snapshot)",
                s::label("export:"), a.name);
            live_export(&a.name, &disk_src, &staged_disk)?;
        }
        ResolvedMode::Offline => {
            println!("{} exporting '{}' (offline)", s::label("export:"), a.name);
            if was_running {
                stop_and_wait(&a.name)?;
            }
            cmd::run_inherit("qemu-img", [
                "convert", "-p", "-O", "qcow2",
                disk_src.to_string_lossy().as_ref(),
                staged_disk.to_string_lossy().as_ref(),
            ])?;
        }
    }

    // ── stage the rest ────────────────────────────────────────────────────────
    if cfg.vm_seed_iso(&a.name).exists() {
        fs::copy(cfg.vm_seed_iso(&a.name), &staged_iso)?;
    }
    let vmuser_path = cfg.vm_ci_dir(&a.name).join(".vmuser");
    if vmuser_path.exists() {
        fs::copy(&vmuser_path, &staged_vmu)?;
    }
    let xml = libvirt::dominfo(&a.name).ok();
    if let Ok(d) = cmd::run("virsh", ["dumpxml", &a.name]) {
        fs::write(&staged_xml, d)?;
    } else if let Some(d) = xml {
        // dominfo is not dumpxml but it's better than nothing for forensics.
        fs::write(&staged_xml, d)?;
    }

    // SHA-256 of the staged disk (single pass over the file).
    let digest = sha256_file(&staged_disk)?;
    let distro_hint = guess_distro_from_seed(&staged_iso).unwrap_or_default();

    let meta = render_meta(MetaArgs {
        name: &a.name,
        arch: &arch,
        qvm_version: env!("CARGO_PKG_VERSION"),
        export_mode: match resolved_mode {
            ResolvedMode::Live    => "live",
            ResolvedMode::Offline => "offline",
        },
        source_host: &hostname_or_unknown(),
        distro_hint: &distro_hint,
        cpus,
        memory_gb: mem_gb,
        disk_gb,
        disk_sha256: &digest,
    });
    fs::write(&staged_meta, meta)?;

    // ── tar it up ─────────────────────────────────────────────────────────────
    if let Some(parent) = a.out.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    println!("{} writing {}", s::label("tar:"), a.out.display());
    cmd::run_inherit("tar", [
        "-cf", a.out.to_string_lossy().as_ref(),
        "-C", staging.to_string_lossy().as_ref(),
        ".",
    ])?;

    // Restart if offline mode stopped a running VM.
    if matches!(resolved_mode, ResolvedMode::Offline) && was_running {
        println!("{} restarting '{}'", s::label("restart:"), a.name);
        libvirt::start(&a.name)?;
    }

    // Retention.
    if let (Some(dir), Some(n)) = (a.rotate_dir.as_ref(), a.keep) {
        rotate_dir(&a.name, dir, n)?;
    }

    let bytes = fs::metadata(&a.out).map(|m| m.len()).unwrap_or(0);
    println!("{} {} ({})",
        s::ok("✓"), a.out.display(), human_bytes(bytes));
    Ok(())
}

enum ResolvedMode { Live, Offline }

// ── live mode steps ──────────────────────────────────────────────────────────

fn live_export(name: &str, disk_src: &Path, staged_disk: &Path) -> Result<()> {
    // 1. Take a disk-only snapshot. Overlay file captures writes during export.
    let overlay = disk_src.with_extension("export-overlay.qcow2");
    let _ = fs::remove_file(&overlay);

    let diskspec = format!("vda,file={}", overlay.display());
    cmd::run("virsh", [
        "snapshot-create-as", name, "_qvm_export",
        "--disk-only", "--quiesce", "--no-metadata",
        "--diskspec", &diskspec,
    ]).map_err(|e| Error::User(format!(
        "live snapshot failed: {e}\n  - rerun with --stop for the offline path."
    )))?;

    // 2. Convert the now-read-only original. If this fails we still need to pivot.
    let convert_res = cmd::run_inherit("qemu-img", [
        "convert", "-p", "-O", "qcow2",
        disk_src.to_string_lossy().as_ref(),
        staged_disk.to_string_lossy().as_ref(),
    ]);

    // 3. Always attempt to pivot back — leaving the overlay live is worse
    //    than aborting the export.
    let pivot_res = cmd::run_inherit("virsh", [
        "blockcommit", name, "vda",
        "--active", "--pivot",
        "--base", disk_src.to_string_lossy().as_ref(),
        "--top",  overlay.to_string_lossy().as_ref(),
    ]);

    // Now reconcile.
    if convert_res.is_err() {
        // Convert failed: pivot may have succeeded or not. Try to abort any
        // in-progress block job so the VM isn't stuck.
        let _ = cmd::run("virsh", ["blockjob", name, "vda", "--abort"]);
        let _ = fs::remove_file(&overlay);
        return Err(Error::User(
            "live export: disk convert failed mid-snapshot. Rerun with --stop.".into()
        ));
    }
    if pivot_res.is_err() {
        // Convert succeeded but pivot failed: the VM is still on the overlay.
        // Surface this loudly — manual recovery is needed.
        return Err(Error::User(format!(
            "live export: snapshot was taken and disk was converted, but \
             blockcommit --pivot failed. The VM is RUNNING ON THE OVERLAY at \
             {}. Recover with:\n  \
             virsh blockjob {name} vda --abort\n  \
             qvm stop {name} ; qvm start {name}\n\
             Or merge manually: virsh blockcommit {name} vda --active --pivot.",
             overlay.display()
        )));
    }
    let _ = fs::remove_file(&overlay);
    Ok(())
}

fn stop_and_wait(name: &str) -> Result<()> {
    libvirt::shutdown(name)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while libvirt::is_running(name) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
    }
    if libvirt::is_running(name) {
        println!("Graceful shutdown timed out; forcing power-off.");
        let _ = libvirt::destroy(name);
    }
    Ok(())
}

fn guest_agent_alive(name: &str) -> bool {
    cmd::run("virsh", [
        "qemu-agent-command", name,
        r#"{"execute":"guest-ping"}"#,
        "--timeout", "5",
    ]).is_ok()
}

fn require_guest_agent(name: &str) -> Result<()> {
    if guest_agent_alive(name) { return Ok(()); }
    Err(Error::User(format!(
        "qemu-guest-agent is not responsive in '{name}'.\n  \
         - install/start qemu-guest-agent in the guest, or\n  \
         - rerun with --stop for offline export."
    )))
}

// ── metadata + hashing ───────────────────────────────────────────────────────

struct MetaArgs<'a> {
    name: &'a str,
    arch: &'a str,
    qvm_version: &'a str,
    export_mode: &'a str,
    source_host: &'a str,
    distro_hint: &'a str,
    cpus: u32,
    memory_gb: u32,
    disk_gb: u32,
    disk_sha256: &'a str,
}

fn render_meta(m: MetaArgs<'_>) -> String {
    format!(
"name         = \"{name}\"
arch         = \"{arch}\"
qvm_version  = \"{ver}\"
exported_at  = \"{stamp}\"
export_mode  = \"{mode}\"
source_host  = \"{host}\"
distro_hint  = \"{distro}\"
cpus         = {cpus}
memory_gb    = {mem}
disk_gb      = {disk}
disk_sha256  = \"{digest}\"
",
        name   = m.name,
        arch   = m.arch,
        ver    = m.qvm_version,
        stamp  = now_rfc3339(),
        mode   = m.export_mode,
        host   = m.source_host,
        distro = m.distro_hint,
        cpus   = m.cpus,
        mem    = m.memory_gb,
        disk   = m.disk_gb,
        digest = m.disk_sha256,
    )
}

fn sha256_file(p: &Path) -> Result<String> {
    let mut f = fs::File::open(p)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

// ── dominfo extraction (best-effort) ─────────────────────────────────────────

fn dominfo_cpus(name: &str) -> Option<u32> {
    let raw = libvirt::dominfo(name).ok()?;
    for line in raw.lines() {
        if let Some(v) = line.trim().strip_prefix("CPU(s):") {
            return v.trim().parse().ok();
        }
    }
    None
}

fn dominfo_memory_gb(name: &str) -> Option<u32> {
    let raw = libvirt::dominfo(name).ok()?;
    // virsh prints "Max memory:     4194304 KiB"
    for line in raw.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Max memory:") {
            let kib: u64 = rest.split_whitespace().next()?.parse().ok()?;
            // KiB → GB (round up to avoid storing 3 GB for a 4 GB VM)
            let gb = kib.div_ceil(1024 * 1024);
            return Some(gb as u32);
        }
    }
    None
}

fn qemu_img_virtual_size_gb(disk: &Path) -> Option<u32> {
    let out = cmd::run("qemu-img", [
        "info", "--output=human", disk.to_string_lossy().as_ref(),
    ]).ok()?;
    for line in out.lines() {
        // Either "virtual size: 50 GiB (...)" or "virtual size: 53687091200 bytes"
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("virtual size:") {
            // Look for "<n> GiB" first.
            if let Some(gib_idx) = rest.find(" GiB") {
                let n_str: String = rest[..gib_idx].chars()
                    .filter(|c| c.is_ascii_digit() || *c == '.').collect();
                if let Ok(n) = n_str.trim().parse::<f64>() {
                    return Some(n.ceil() as u32);
                }
            }
            // Fall back: "<n> bytes (...)" or just the byte count.
            let n_str: String = rest.split_whitespace().next()?.chars()
                .filter(|c| c.is_ascii_digit()).collect();
            if let Ok(bytes) = n_str.parse::<u64>() {
                let gib = bytes.div_ceil(1024u64.pow(3));
                return Some(gib as u32);
            }
        }
    }
    None
}

fn host_arch() -> String {
    cmd::run("uname", ["-m"]).map(|s| s.trim().to_string()).unwrap_or_else(|_| "unknown".into())
}

fn hostname_or_unknown() -> String {
    cmd::run("hostname", Vec::<&str>::new()).map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

/// Crude RFC3339 timestamp without bringing in chrono. Uses `date -u
/// +%Y-%m-%dT%H:%M:%SZ` from the host (universally present on Linux).
fn now_rfc3339() -> String {
    cmd::run("date", ["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

/// Read the cloud-init user-data inside the seed ISO and try to find the
/// `# distro: foo:ver` hint qvm always emits. Best-effort; returns "" on
/// any failure.
fn guess_distro_from_seed(_iso: &Path) -> Option<String> {
    // Parsing an ISO9660 from Rust would be heavy. cloud-init ISOs are tiny,
    // so `strings` would work — but we don't depend on strings. Skip for
    // now; meta gets a blank distro_hint. The disk's qcow2 backing format
    // isn't enough to reconstruct it; the user can still pass --bridge on
    // import. Future: parse the seed ISO properly.
    None
}

fn human_bytes(n: u64) -> String {
    let n = n as f64;
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    if      n >= GB { format!("{:.2} GB", n / GB) }
    else if n >= MB { format!("{:.1} MB", n / MB) }
    else if n >= KB { format!("{:.1} KB", n / KB) }
    else            { format!("{n} B") }
}

// ── staging dir lifecycle ────────────────────────────────────────────────────

fn mkstaging(name: &str) -> Result<PathBuf> {
    let base = std::env::temp_dir().join(format!("qvm-export-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base)?;
    Ok(base)
}

struct StagingGuard<'a>(&'a Path);
impl Drop for StagingGuard<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0);
    }
}

// ── retention (item 11) ──────────────────────────────────────────────────────

fn rotate_dir(name: &str, dir: &Path, keep: u32) -> Result<()> {
    if keep == 0 {
        return Err(Error::User("--keep must be >= 1".into()));
    }
    let prefix = format!("{name}.qvm.tar");
    let mut entries: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for ent in rd.flatten() {
            let p = ent.path();
            let fname = match p.file_name().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            if !fname.starts_with(&prefix) { continue; }
            let mtime = ent.metadata().and_then(|m| m.modified()).ok();
            if let Some(t) = mtime {
                entries.push((p, t));
            }
        }
    }
    if entries.len() <= keep as usize { return Ok(()); }
    // Newest first.
    entries.sort_by_key(|e| std::cmp::Reverse(e.1));
    let drop_list = &entries[keep as usize ..];
    println!("Retention: keeping newest {}, removing {}:", keep, drop_list.len());
    for (p, _) in drop_list {
        println!("  - {}", p.display());
        let _ = fs::remove_file(p);
    }
    Ok(())
}

// ── re-export so import.rs can read the metadata file ───────────────────────

/// Field set from `qvm-meta.toml`. Parsed defensively: every field has
/// a fallback so old/partial tarballs still import.
#[derive(Debug, Default, Clone)]
pub struct Meta {
    pub name:         String,
    pub arch:         String,
    pub qvm_version:  String,
    pub exported_at:  String,
    pub export_mode:  String,
    pub source_host:  String,
    pub distro_hint:  String,
    pub cpus:         Option<u32>,
    pub memory_gb:    Option<u32>,
    pub disk_gb:      Option<u32>,
    pub disk_sha256:  Option<String>,
}

impl Meta {
    pub fn parse(s: &str) -> Self {
        let mut m = Meta::default();
        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let (k, v) = match line.split_once('=') {
                Some(kv) => kv,
                None => continue,
            };
            let key = k.trim();
            let val = v.trim();
            let str_val = || val.trim_matches('"').to_string();
            let num_val = || val.parse::<u32>().ok();
            match key {
                "name"         => m.name        = str_val(),
                "arch"         => m.arch        = str_val(),
                "qvm_version"  => m.qvm_version = str_val(),
                "exported_at"  => m.exported_at = str_val(),
                "export_mode"  => m.export_mode = str_val(),
                "source_host"  => m.source_host = str_val(),
                "distro_hint"  => m.distro_hint = str_val(),
                "cpus"         => m.cpus      = num_val(),
                "memory_gb"    => m.memory_gb = num_val(),
                "disk_gb"      => m.disk_gb   = num_val(),
                "disk_sha256"  => m.disk_sha256 = Some(str_val()),
                _ => {}
            }
        }
        m
    }
}

