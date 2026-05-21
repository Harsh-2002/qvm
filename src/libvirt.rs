//! All libvirt access goes through here. Pure virsh shell-outs.
//!
//! Libvirt is the source of truth for "does this VM exist". We never
//! maintain a separate VM list.

use crate::cmd::{require, run};
use crate::error::{Error, Result};
use crate::util;

pub fn require_virsh() -> Result<()> { require("virsh") }

/// True if the domain is defined (regardless of running state).
pub fn exists(name: &str) -> bool {
    run("virsh", ["dominfo", name]).is_ok()
}

/// True if the domain is currently running.
pub fn is_running(name: &str) -> bool {
    match run("virsh", ["domstate", name]) {
        Ok(s) => s.trim().eq_ignore_ascii_case("running"),
        Err(_) => false,
    }
}

pub fn start(name: &str)    -> Result<()> { run("virsh", ["start",    name]).map(drop) }
pub fn shutdown(name: &str) -> Result<()> { run("virsh", ["shutdown", name]).map(drop) }
pub fn reboot(name: &str)   -> Result<()> { run("virsh", ["reboot",   name]).map(drop) }
pub fn destroy(name: &str)  -> Result<()> { run("virsh", ["destroy",  name]).map(drop) }
pub fn autostart_on(name: &str) -> Result<()> { run("virsh", ["autostart", name]).map(drop) }

/// Undefine + remove NVRAM. We do the disk file removal ourselves
/// (we manage those files explicitly).
pub fn undefine(name: &str) -> Result<()> {
    // Try with --nvram first (UEFI VMs); fall back for BIOS / very old libvirt.
    if run("virsh", ["undefine", name, "--nvram"]).is_ok() { return Ok(()); }
    run("virsh", ["undefine", name]).map(drop)
}

/// `virsh list --all`, raw output for `qvm ls`.
pub fn list_all() -> Result<String> { run("virsh", ["list", "--all"]) }

/// `virsh dominfo <name>` raw.
pub fn dominfo(name: &str) -> Result<String> { run("virsh", ["dominfo", name]) }

/// Get IPv4 of a VM via the QEMU guest agent (best), DHCP lease, or ARP table.
///
/// Skips the loopback interface so a `lo 127.0.0.1/8` line never wins.
pub fn ipv4(name: &str) -> Option<String> {
    for src in ["agent", "lease", "arp"] {
        if let Ok(out) = run("virsh", ["domifaddr", name, "--source", src]) {
            for line in out.lines() {
                if !line.contains("ipv4") { continue; }
                let mut cols = line.split_whitespace();
                let iface = cols.next().unwrap_or("");
                // virsh repeats the iface name on the first row only; subsequent rows
                // start with the protocol column. Treat "-" (continuation) as "carry over".
                if iface == "lo" { continue; }
                // Address column is index 3 on a leading row, index 2 on a continuation row.
                let addr = if iface == "ipv4" || iface == "ipv6" {
                    line.split_whitespace().nth(1)
                } else {
                    line.split_whitespace().nth(3)
                };
                if let Some(addr) = addr {
                    if let Some(ip) = addr.split('/').next() {
                        if !ip.is_empty() && ip != "127.0.0.1" {
                            return Some(ip.into());
                        }
                    }
                }
            }
        }
    }
    None
}

/// `virsh vncdisplay <name>` → ":1" (means port 5901). None if not started or no VNC.
pub fn vnc_display(name: &str) -> Option<u16> {
    let out = run("virsh", ["vncdisplay", name]).ok()?;
    let s = out.trim();
    let after_colon = s.rsplit(':').next()?;
    let n: u16 = after_colon.parse().ok()?;
    n.checked_add(5900)
}

// ── precondition helpers ──────────────────────────────────────────────────────

/// Standard prelude: virsh available + name valid + domain exists.
/// Use this at the top of any command that operates on an existing VM.
pub fn require_defined(name: &str) -> Result<()> {
    require_virsh()?;
    util::require_name(name)?;
    if !exists(name) {
        return Err(Error::User(format!("VM '{name}' not found.")));
    }
    Ok(())
}

/// Like [`require_defined`], plus the VM must currently be running.
pub fn require_running(name: &str) -> Result<()> {
    require_defined(name)?;
    if !is_running(name) {
        return Err(Error::User(format!("'{name}' is not running.")));
    }
    Ok(())
}

/// Used by `qvm create`: domain must NOT already exist.
pub fn require_absent(name: &str) -> Result<()> {
    require_virsh()?;
    util::require_name(name)?;
    if exists(name) {
        return Err(Error::User(format!("VM '{name}' already exists.")));
    }
    Ok(())
}
