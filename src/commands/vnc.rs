use crate::cmd::{have, run as cmd_run};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt;
use std::process::Command;

pub fn run(cfg: &Config, name: &str, open: bool) -> Result<()> {
    libvirt::require_defined(name)?;
    if !libvirt::is_running(name) {
        return Err(Error::User(format!(
            "'{name}' is not running. Start it first: qvm start {name}"
        )));
    }
    let port = libvirt::vnc_display(name).ok_or_else(||
        Error::User(format!("'{name}' has no VNC display configured.")))?;

    let bind = &cfg.vnc.bind;
    let host = host_label().unwrap_or_else(|| "this-host".into());

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
        // 0.0.0.0 means "listen on all interfaces" — a local viewer should
        // still connect via loopback, not the literal 0.0.0.0.
        let target_host = if bind == "0.0.0.0" { "127.0.0.1" } else { bind.as_str() };
        let target = format!("{target_host}:{port}");

        for prog in ["remote-viewer", "vncviewer", "tigervnc-viewer", "vinagre"] {
            if have(prog) {
                let _ = Command::new(prog).arg(&target).spawn();
                return Ok(());
            }
        }
        eprintln!("(no local viewer found - copy the command above to your laptop)");
    }
    Ok(())
}

fn host_label() -> Option<String> {
    let s = cmd_run("hostname", std::iter::empty::<&str>()).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
