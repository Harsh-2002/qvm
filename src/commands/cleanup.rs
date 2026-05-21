//! `qvm cleanup` — find and remove leftover files from VMs that no longer
//! exist in libvirt.
//!
//! Scenario: `qemu-img convert` finished but `virt-install` hadn't started
//! when qvm was killed (host reboot, SIGKILL, network drop). Result: a qcow2
//! file in `<vms>/` with no matching libvirt domain. Same for the cloud-init
//! seed ISO and `<cloudinit>/<name>/` working dirs.
//!
//! Source of truth for "does this VM exist" is still libvirt (`virsh list
//! --all --name`). Anything on disk whose basename isn't in that list is an
//! orphan — by definition unreachable through qvm.

use crate::config::Config;
use crate::error::Result;
use crate::libvirt;
use crate::util::confirm_phrase;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrphanKind {
    Disk,        // <vms>/<name>.qcow2
    SeedIso,     // <cloudinit>/<name>.iso
    SeedDir,     // <cloudinit>/<name>/
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    pub name: String,
    pub kind: OrphanKind,
    pub path: PathBuf,
    /// Bytes on disk. None for directories (the in-process walk would be
    /// expensive and the count alone is what users want).
    pub size: Option<u64>,
}

/// Walk the configured storage dirs and return every file/dir that doesn't
/// correspond to a libvirt-defined VM. `live_names` is the set returned by
/// `libvirt::domains()` — passed in so callers (TUI, CLI) can share a single
/// virsh call when they need both pieces of information.
pub fn scan_with(cfg: &Config, live_names: &HashSet<String>) -> Vec<Orphan> {
    let mut out = Vec::new();

    // Orphan disks
    if let Ok(rd) = fs::read_dir(&cfg.paths.vms) {
        for ent in rd.flatten() {
            let p = ent.path();
            // We manage `*.qcow2` and the transient `*.partial` files. Any
            // other extension (.bak, .iso, .lock) is not ours; leave it.
            let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
            if ext != "qcow2" && ext != "partial" { continue; }
            let stem = match p.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            // `.partial` strips one extension; if the user had a name like
            // foo.qcow2.partial, file_stem yields "foo.qcow2" which is fine.
            let name = stem.trim_end_matches(".qcow2").to_string();
            if live_names.contains(&name) { continue; }
            let size = ent.metadata().ok().map(|m| m.len());
            out.push(Orphan { name, kind: OrphanKind::Disk, path: p, size });
        }
    }

    // Orphan seed ISOs and seed dirs
    if let Ok(rd) = fs::read_dir(&cfg.paths.cloudinit) {
        for ent in rd.flatten() {
            let p = ent.path();
            let meta = match ent.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_file() {
                if p.extension().and_then(|s| s.to_str()) != Some("iso") { continue; }
                let stem = match p.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                if live_names.contains(&stem) { continue; }
                out.push(Orphan {
                    name: stem,
                    kind: OrphanKind::SeedIso,
                    path: p,
                    size: Some(meta.len()),
                });
            } else if meta.is_dir() {
                let name = match p.file_name().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                if live_names.contains(&name) { continue; }
                out.push(Orphan {
                    name,
                    kind: OrphanKind::SeedDir,
                    path: p,
                    size: None,
                });
            }
        }
    }

    out
}

/// Convenience wrapper that fetches the live-domain set itself.
pub fn scan(cfg: &Config) -> Result<Vec<Orphan>> {
    let live: HashSet<String> = libvirt::domains()?
        .into_iter()
        .map(|d| d.name)
        .collect();
    Ok(scan_with(cfg, &live))
}

pub fn run(cfg: &Config, force: bool) -> Result<()> {
    use crate::style as s;
    let orphans = scan(cfg)?;

    if orphans.is_empty() {
        println!("{} no orphan files found.", s::ok("✓"));
        return Ok(());
    }

    println!("{} {} orphan file(s):", s::warn("!"), orphans.len());
    for o in &orphans {
        let kind = match o.kind {
            OrphanKind::Disk    => "disk",
            OrphanKind::SeedIso => "seed-iso",
            OrphanKind::SeedDir => "seed-dir",
        };
        let size = match o.size {
            Some(bytes) => format_bytes(bytes),
            None => "-".into(),
        };
        println!("  {:<8}  {:<24}  {:<10}  {}",
            s::dim(kind), o.name, size, o.path.display());
    }

    if !force && !confirm_phrase("Remove all of the above? Type 'yes':", "yes") {
        println!("Cancelled. Nothing removed.");
        return Ok(());
    }

    let mut removed = 0usize;
    let mut failed: Vec<(PathBuf, String)> = Vec::new();
    for o in &orphans {
        let res = match o.kind {
            OrphanKind::SeedDir => fs::remove_dir_all(&o.path),
            _                   => fs::remove_file(&o.path),
        };
        match res {
            Ok(()) => removed += 1,
            Err(e) => failed.push((o.path.clone(), e.to_string())),
        }
    }

    if failed.is_empty() {
        println!("{} removed {} orphan(s).", s::ok("✓"), removed);
    } else {
        println!("{} removed {}; {} failed:", s::warn("!"), removed, failed.len());
        for (p, e) in &failed {
            println!("  {}  {}", p.display(), e);
        }
    }
    Ok(())
}

fn format_bytes(b: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = b as f64;
    if      b >= GB { format!("{:.1} GB", b / GB) }
    else if b >= MB { format!("{:.1} MB", b / MB) }
    else if b >= KB { format!("{:.1} KB", b / KB) }
    else            { format!("{b} B")            }
}
