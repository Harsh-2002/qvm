//! `qvm flatten <name>` — detach a qcow2 from its backing file.
//!
//! Migration helper for VMs created by the bash predecessor, which used
//! `qemu-img create -b <base>` to make thin clones. Those VMs corrupt
//! silently if the base file changes — exactly the disaster qvm was built
//! to avoid.
//!
//! `qemu-img convert` always reads every block from the chain, so the
//! output file is unconditionally self-contained. We write to a sibling
//! `.flat.qcow2`, then atomically rename it over the original.

use crate::cmd;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt;
use std::fs;

pub fn run(cfg: &Config, name: &str) -> Result<()> {
    libvirt::require_defined(name)?;
    let disk = cfg.vm_disk(name);
    if !disk.exists() {
        return Err(Error::User(format!(
            "disk not found: {}\n  - was this VM created outside qvm? Use `qvm cleanup` first.",
            disk.display()
        )));
    }

    // Pre-flight: is there actually a backing file to detach?
    let info = cmd::run("qemu-img", ["info", disk.to_string_lossy().as_ref()])?;
    let has_backing = info.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with("backing file:") || l.starts_with("backing file format:")
    });
    if !has_backing {
        return Err(Error::User(format!(
            "{} is already self-contained — no backing file to detach.",
            disk.display()
        )));
    }

    // Stop the VM cleanly if running so qemu-img can read the file without
    // racing the live writer. Track whether we stopped it so we can restart.
    let was_running = libvirt::is_running(name);
    if was_running {
        println!("Stopping '{name}' for flatten…");
        libvirt::shutdown(name)?;
        // Wait up to 60s for shutdown, then force-destroy.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while libvirt::is_running(name) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        if libvirt::is_running(name) {
            println!("Graceful shutdown timed out; forcing power-off.");
            let _ = libvirt::destroy(name);
        }
    }

    // Convert into a sibling temp file, then rename atomically (same FS).
    let tmp = disk.with_extension("flat.qcow2");
    let _ = fs::remove_file(&tmp);
    println!("Flattening {} → {} …", disk.display(), tmp.display());
    cmd::run_inherit("qemu-img", [
        "convert", "-O", "qcow2",
        disk.to_string_lossy().as_ref(),
        tmp.to_string_lossy().as_ref(),
    ])?;
    fs::rename(&tmp, &disk)?;

    if was_running {
        println!("Restarting '{name}'…");
        libvirt::start(name)?;
    }

    println!("Flattened '{name}': disk is now self-contained.");
    Ok(())
}
