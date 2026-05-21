use crate::cloudinit::login_user_of;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt;
use crate::style;

pub fn ls() -> Result<()> {
    libvirt::require_virsh()?;
    let doms = libvirt::domains()?;
    // Header: dim small-caps style.
    println!(
        "  {}  {}  {}",
        style::label(format!("{:>3}", "ID")),
        style::label(format!("{:<20}", "NAME")),
        style::label("STATE"),
    );
    if doms.is_empty() {
        println!("  {}", style::dim("(no VMs defined)"));
        return Ok(());
    }
    for d in &doms {
        let id_col = match d.id {
            Some(n) => format!("{n:>3}"),
            None    => "  -".into(),
        };
        println!(
            "  {}  {}  {}",
            style::dim(id_col),
            d.name,
            style::state_styled(&d.state),
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
