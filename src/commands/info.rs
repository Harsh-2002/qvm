use crate::cloudinit::login_user_of;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt;
use crate::util::require_name;

pub fn ls() -> Result<()> {
    libvirt::require_virsh()?;
    print!("{}", libvirt::list_all()?);
    Ok(())
}

pub fn inspect(name: &str) -> Result<()> {
    libvirt::require_virsh()?;
    require_name(name)?;
    if !libvirt::exists(name) {
        return Err(Error::User(format!("VM '{name}' not found.")));
    }
    print!("{}", libvirt::dominfo(name)?);
    Ok(())
}

pub fn ip(name: &str) -> Result<()> {
    libvirt::require_virsh()?;
    require_name(name)?;
    if !libvirt::exists(name) { return Err(Error::User(format!("VM '{name}' not found."))); }
    if !libvirt::is_running(name) { return Err(Error::User(format!("'{name}' is not running."))); }
    match libvirt::ipv4(name) {
        Some(ip) => { println!("{ip}"); Ok(()) }
        None     => Err(Error::User("no IP yet - VM may still be booting; retry in ~30s.".into())),
    }
}

pub fn ssh_cmd(cfg: &Config, name: &str) -> Result<()> {
    libvirt::require_virsh()?;
    require_name(name)?;
    if !libvirt::exists(name) { return Err(Error::User(format!("VM '{name}' not found."))); }
    if !libvirt::is_running(name) { return Err(Error::User(format!("'{name}' is not running."))); }
    let ip = libvirt::ipv4(name).ok_or_else(||
        Error::User("no IP yet - VM may still be booting.".into()))?;
    match login_user_of(cfg, name) {
        Some(u) => println!("ssh {u}@{ip}"),
        None    => println!("ssh <user>@{ip}    # login user unknown - was this VM created by qvm?"),
    }
    Ok(())
}
