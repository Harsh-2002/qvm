use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt;
use crate::util::require_name;
use std::fs;
use std::io::{self, Write};

pub fn run(cfg: &Config, name: &str, force: bool) -> Result<()> {
    libvirt::require_virsh()?;
    require_name(name)?;
    if !libvirt::exists(name) {
        return Err(Error::User(format!("VM '{name}' does not exist.")));
    }

    if !force {
        print!("Delete '{name}' and all its data? Type 'yes': ");
        io::stdout().flush().ok();
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        if line.trim() != "yes" {
            println!("Cancelled.");
            return Ok(());
        }
    }

    if libvirt::is_running(name) { let _ = libvirt::destroy(name); }
    // undefine() handles UEFI NVRAM internally
    let _ = libvirt::undefine(name);

    let disk = cfg.paths.vms.join(format!("{name}.qcow2"));
    let iso  = cfg.paths.cloudinit.join(format!("{name}.iso"));
    let ci   = cfg.paths.cloudinit.join(name);
    let _ = fs::remove_file(&disk);
    let _ = fs::remove_file(&iso);
    let _ = fs::remove_dir_all(&ci);

    if libvirt::exists(name) {
        return Err(Error::User(format!(
            "'{name}' is still defined in libvirt.\nRun manually: virsh undefine {name} --nvram --remove-all-storage"
        )));
    }
    println!("Deleted '{name}'.");
    Ok(())
}
