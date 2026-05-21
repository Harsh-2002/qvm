//! All libvirt access goes through here. Pure virsh shell-outs.
//!
//! Libvirt is the source of truth for "does this VM exist". We never
//! maintain a separate VM list.

use crate::cmd::{require, run};
use crate::error::Result;

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

/// Get IPv4 of a VM via the QEMU guest agent (best) or DHCP lease.
pub fn ipv4(name: &str) -> Option<String> {
    for src in ["agent", "lease", "arp"] {
        if let Ok(out) = run("virsh", ["domifaddr", name, "--source", src]) {
            for line in out.lines() {
                if line.contains("ipv4") {
                    if let Some(addr) = line.split_whitespace().nth(3) {
                        if let Some(ip) = addr.split('/').next() {
                            if !ip.is_empty() { return Some(ip.into()); }
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
    Some(5900 + n)
}
