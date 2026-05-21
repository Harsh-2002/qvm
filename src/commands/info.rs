use crate::cloudinit::login_user_of;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt;
use crate::style;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct LsRow {
    pub name:  String,
    pub state: String,
    pub id:    Option<u32>,
    pub ip:    Option<String>,
}

pub fn ls(json: bool) -> Result<()> {
    libvirt::require_virsh()?;
    let doms = libvirt::domains()?;
    let rows: Vec<LsRow> = doms.iter().map(|d| LsRow {
        name:  d.name.clone(),
        state: d.state.clone(),
        id:    d.id,
        // Skip IP lookup unless we'll print it. Running VMs only — same as
        // the TUI's behavior; ipv4() is up to 3 shell-outs.
        ip:    if d.state == "running" { libvirt::ipv4(&d.name) } else { None },
    }).collect();

    if json {
        let out = serde_json::to_string_pretty(&rows)
            .map_err(|e| Error::User(format!("json encode: {e}")))?;
        println!("{out}");
        return Ok(());
    }

    println!(
        "  {}  {}  {}  {}",
        style::label(format!("{:>3}", "ID")),
        style::label(format!("{:<20}", "NAME")),
        style::label(format!("{:<10}", "STATE")),
        style::label("IP"),
    );
    if rows.is_empty() {
        println!("  {}", style::dim("(no VMs defined)"));
        return Ok(());
    }
    for r in &rows {
        let id_col = match r.id {
            Some(n) => format!("{n:>3}"),
            None    => "  -".into(),
        };
        let ip_col = r.ip.as_deref().unwrap_or("-");
        println!(
            "  {}  {:<20}  {:<10}  {}",
            style::dim(id_col),
            r.name,
            style::state_styled(&r.state),
            style::dim(ip_col),
        );
    }
    Ok(())
}

pub fn inspect(name: &str) -> Result<()> {
    libvirt::require_defined(name)?;
    print!("{}", libvirt::dominfo(name)?);
    Ok(())
}

pub fn ip(name: &str) -> Result<()> {
    libvirt::require_running(name)?;
    match libvirt::ipv4(name) {
        Some(ip) => { println!("{ip}"); Ok(()) }
        None     => Err(Error::User("no IP yet - VM may still be booting; retry in ~30s.".into())),
    }
}

pub fn ssh_cmd(cfg: &Config, name: &str) -> Result<()> {
    libvirt::require_running(name)?;
    let ip = libvirt::ipv4(name).ok_or_else(||
        Error::User("no IP yet - VM may still be booting.".into()))?;
    match login_user_of(cfg, name) {
        Some(u) => println!("ssh {u}@{ip}"),
        None    => println!("ssh <user>@{ip}    # login user unknown - was this VM created by qvm?"),
    }
    Ok(())
}

/// `qvm ssh <vm>` — actually exec ssh into the VM. Resolves login user
/// from the cloudinit sidecar and IP via libvirt; replaces the qvm
/// process with ssh so the user lands directly in a shell.
pub fn ssh_exec(cfg: &Config, name: &str) -> Result<()> {
    libvirt::require_running(name)?;
    let ip = libvirt::ipv4(name).ok_or_else(||
        Error::User("no IP yet - VM may still be booting.".into()))?;
    let user = login_user_of(cfg, name).ok_or_else(||
        Error::User(format!(
            "login user for '{name}' unknown — was this VM created by qvm?\n  \
             Use `ssh <user>@{ip}` directly."
        )))?;
    let target = format!("{user}@{ip}");
    crate::cmd::exec("ssh", [target.as_str()])
}
