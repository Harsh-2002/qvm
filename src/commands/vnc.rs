use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt;
use crate::util::require_name;
use std::process::Command;

pub fn run(cfg: &Config, name: &str, open: bool) -> Result<()> {
    libvirt::require_virsh()?;
    require_name(name)?;
    if !libvirt::exists(name) { return Err(Error::User(format!("VM '{name}' not found."))); }
    if !libvirt::is_running(name) {
        return Err(Error::User(format!("'{name}' is not running. Start it first: qvm start {name}")));
    }
    let port = libvirt::vnc_display(name).ok_or_else(||
        Error::User(format!("'{name}' has no VNC display configured.")))?;

    let bind = &cfg.vnc.bind;
    let host = match host_label() { Some(h) => h, None => "this-host".into() };

    println!("VNC for '{name}':");
    println!("  bind   : {bind}");
    println!("  port   : {port}");
    println!();
    if bind == "127.0.0.1" {
        println!("From your laptop:");
        println!("  ssh -L {port}:127.0.0.1:{port} root@{host}");
        println!("  # then in another terminal / vnc viewer:");
        println!("  vncviewer 127.0.0.1:{port}");
    } else {
        println!("Connect from any LAN host:");
        println!("  vncviewer {bind}:{port}");
        println!("(no password by default - set one with `virsh edit {name}` if exposing on LAN)");
    }

    if open {
        // Best-effort local launch. Tries common viewers; not fatal if none found.
        for prog in ["remote-viewer", "vncviewer", "tigervnc-viewer", "vinagre"] {
            if Command::new("sh").arg("-c").arg(format!("command -v {prog}"))
                .status().map(|s| s.success()).unwrap_or(false)
            {
                let target = format!("{bind}:{port}");
                let _ = Command::new(prog).arg(&target).spawn();
                return Ok(());
            }
        }
        eprintln!("(no local viewer found - copy the command above to your laptop)");
    }
    Ok(())
}

fn host_label() -> Option<String> {
    let out = Command::new("hostname").output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
