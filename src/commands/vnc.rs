use crate::cmd::{have, run as cmd_run};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt::{self, VncEndpoint};
use std::process::Command;

pub fn run(cfg: &Config, name: &str, open: bool) -> Result<()> {
    libvirt::require_running(name)?;
    let ep = libvirt::vnc_endpoint(name).ok_or_else(||
        Error::User(format!("'{name}' has no VNC display configured.")))?;
    let bind = &cfg.vnc.bind;

    print_info(name, bind, ep);

    if open {
        // 0.0.0.0 means "listen on all interfaces" — a local viewer should
        // still dial loopback, not the literal 0.0.0.0.
        let target_host = if bind == "0.0.0.0" { "127.0.0.1" } else { bind.as_str() };
        // Use the double-colon explicit-port form — every modern viewer
        // accepts it, where `host:port` is misread as `host:display`.
        let target = format!("{target_host}::{}", ep.port);

        for prog in ["remote-viewer", "vncviewer", "tigervnc-viewer", "vinagre"] {
            if have(prog) {
                let _ = Command::new(prog).arg(&target).spawn();
                return Ok(());
            }
        }
        eprintln!("(no local viewer found — copy a command above to your laptop)");
    }
    Ok(())
}

fn print_info(name: &str, bind: &str, ep: VncEndpoint) {
    let host = host_label().unwrap_or_else(|| "this-host".into());

    println!("VNC for '{name}':");
    println!("  bind     {bind}");
    println!("  display  :{}", ep.display);
    println!("  port     {}", ep.port);
    println!();
    println!("From a VNC viewer:");
    println!("  vncviewer {bind}:{}           # canonical: host:display", ep.display);
    println!("  vncviewer {bind}::{}        # explicit port form (always works)", ep.port);
    println!();
    println!("From macOS Screen Sharing:");
    println!("  open vnc://{bind}");

    if bind == "127.0.0.1" {
        println!();
        println!("Loopback bind — first tunnel via SSH:");
        println!("  ssh -L {p}:127.0.0.1:{p} root@{host}", p = ep.port);
        println!("  # then on your laptop:");
        println!("  open vnc://127.0.0.1");
    } else {
        println!();
        println!("(no VNC password by default — set one with `virsh edit {name}` if exposing on LAN)");
    }
}

fn host_label() -> Option<String> {
    let s = cmd_run("hostname", std::iter::empty::<&str>()).ok()?;
    let s = s.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
