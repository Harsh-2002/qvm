use crate::cmd::{exec, have, require, run as cmd_run};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt::{self, VncEndpoint};
use std::path::Path;
use std::process::Command;

const DEFAULT_WS_PORT: u16 = 6080;
const NOVNC_DIRS: &[&str] = &[
    "/usr/share/novnc",          // Debian, Ubuntu
    "/usr/share/webapps/novnc",  // Alpine
    "/usr/share/noVNC",          // Fedora, Rocky
];

pub fn run(cfg: &Config, name: &str, open: bool, browser: bool) -> Result<()> {
    libvirt::require_running(name)?;
    let ep = libvirt::vnc_endpoint(name).ok_or_else(||
        Error::User(format!("'{name}' has no VNC display configured.")))?;
    let bind = &cfg.vnc.bind;

    if browser {
        return run_browser(name, bind, ep);
    }

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

/// Spin up a noVNC websocket bridge in the foreground and print a URL the
/// user can open in any browser on the LAN. Ctrl-C tears the bridge down.
fn run_browser(name: &str, bind: &str, ep: VncEndpoint) -> Result<()> {
    require("websockify")?;
    let novnc = NOVNC_DIRS.iter()
        .find(|p| Path::new(p).join("vnc_lite.html").exists())
        .copied()
        .ok_or_else(|| Error::User(
            "noVNC not installed. Try: apt-get install novnc  (or your distro's equivalent)".into()
        ))?;

    let dial = if bind == "0.0.0.0" { "127.0.0.1" } else { bind };
    let host_ip = detect_host_ip().unwrap_or_else(|| "<this-host>".into());

    println!("Browser VNC for '{name}':");
    println!("  Open in any browser on this LAN:");
    println!("    http://{host_ip}:{DEFAULT_WS_PORT}/vnc_lite.html?host={host_ip}&port={DEFAULT_WS_PORT}&autoconnect=true&resize=scale&reconnect=true");
    println!();
    println!("Press Ctrl-C to stop the bridge.");
    println!();

    // exec replaces qvm with websockify; on Ctrl-C the shell gets SIGINT
    // and websockify exits cleanly. exec only returns on error.
    exec("websockify", [
        "--web", novnc,
        &format!("0.0.0.0:{DEFAULT_WS_PORT}"),
        &format!("{dial}:{}", ep.port),
    ])
}

/// Best-effort: first non-loopback IPv4 from `hostname -I`.
fn detect_host_ip() -> Option<String> {
    let out = cmd_run("hostname", ["-I"]).ok()?;
    out.split_whitespace()
        .find(|s| !s.starts_with("127.") && s.contains('.'))
        .map(str::to_string)
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
