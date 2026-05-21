//! `qvm snap` — VM snapshot management. Thin wrapper around `virsh
//! snapshot-*`. Five subcommands: create, list, revert, rm, rotate.
//!
//! Internal snapshots (the default) live inside the qcow2 file; no extra
//! file management. External/live snapshots are a future feature (see
//! item 6's live export, which uses the same machinery).

use crate::cmd;
use crate::error::{Error, Result};
use crate::libvirt;

/// `qvm snap create <vm> <snap> [--quiesce]`.
///
/// `--quiesce` flushes the guest's filesystems via qemu-guest-agent before
/// the snapshot, producing a crash-consistent image instead of a possibly
/// dirty one. Requires the agent to be installed and responding in the
/// guest; otherwise virsh errors and we surface that error verbatim.
pub fn create(name: &str, snap: &str, quiesce: bool) -> Result<()> {
    libvirt::require_defined(name)?;
    let mut args: Vec<&str> = vec!["snapshot-create-as", name, snap];
    if quiesce {
        args.push("--quiesce");
    }
    cmd::run_inherit("virsh", args)?;
    println!("Created snapshot '{snap}' on '{name}'.");
    Ok(())
}

/// `qvm snap list <vm>` — print `virsh snapshot-list` output verbatim.
pub fn list(name: &str) -> Result<()> {
    libvirt::require_defined(name)?;
    let out = cmd::run("virsh", ["snapshot-list", name])?;
    print!("{out}");
    Ok(())
}

/// `qvm snap revert <vm> <snap> [--running]`.
///
/// By default `virsh snapshot-revert` restores the VM in the state it was
/// in when the snapshot was taken (often shut off). `--running` forces the
/// VM to be running afterwards regardless.
pub fn revert(name: &str, snap: &str, running: bool) -> Result<()> {
    libvirt::require_defined(name)?;
    let mut args: Vec<&str> = vec!["snapshot-revert", name, snap];
    if running {
        args.push("--running");
    }
    cmd::run_inherit("virsh", args)?;
    println!("Reverted '{name}' to snapshot '{snap}'.");
    Ok(())
}

/// `qvm snap rm <vm> <snap>`.
pub fn remove(name: &str, snap: &str) -> Result<()> {
    libvirt::require_defined(name)?;
    cmd::run_inherit("virsh", ["snapshot-delete", name, snap])?;
    println!("Removed snapshot '{snap}' from '{name}'.");
    Ok(())
}

/// `qvm snap rotate <vm> --keep N` — delete all but the newest N
/// snapshots on `name`. Sort key is creation time as reported by
/// `virsh snapshot-list`.
pub fn rotate(name: &str, keep: u32) -> Result<()> {
    libvirt::require_defined(name)?;
    if keep == 0 {
        return Err(Error::User("--keep must be >= 1".into()));
    }
    let raw = cmd::run("virsh", ["snapshot-list", name])?;
    let mut snaps = parse_snapshot_list(&raw);
    // Newest first. virsh emits oldest first; reverse to keep top N.
    snaps.reverse();
    if snaps.len() <= keep as usize {
        println!("'{name}' has {} snapshot(s); nothing to rotate.", snaps.len());
        return Ok(());
    }
    let (_keep_list, drop_list) = snaps.split_at(keep as usize);
    println!("Keeping newest {}, removing {}:", keep, drop_list.len());
    for s in drop_list {
        println!("  - {}", s);
        cmd::run("virsh", ["snapshot-delete", name, s])?;
    }
    Ok(())
}

/// Parse the `Name` column from `virsh snapshot-list` output. The format
/// is a fixed 3-row header followed by space-separated rows whose first
/// column is the snapshot name.
///
/// Returned vector is in the same order virsh printed it (oldest first).
pub fn parse_snapshot_list(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut lines = raw.lines();
    // Skip the two header lines: `  Name ... Creation Time ... State` then `-----`.
    // Be tolerant: skip leading blank lines and any line that starts with
    // a dash, space-only, or "Name".
    let mut header_done = false;
    for line in lines.by_ref() {
        let t = line.trim();
        if t.is_empty() { continue; }
        if !header_done {
            if t.starts_with("---") { header_done = true; }
            continue;
        }
        if let Some(first) = t.split_whitespace().next() {
            out.push(first.to_string());
        }
    }
    out
}

