//! `qvm console <name>` — drop into the VM's serial console.
//!
//! Thin wrapper around `virsh console`. The TUI's `e` action uses the same
//! call; this exposes it to CLI/script users who don't want to remember the
//! virsh syntax.

use crate::cmd;
use crate::error::Result;
use crate::libvirt;

pub fn run(name: &str) -> Result<()> {
    libvirt::require_running(name)?;
    cmd::run_tty("virsh", ["console", name])
}
