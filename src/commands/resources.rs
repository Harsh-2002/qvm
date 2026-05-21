use crate::cmd::run;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt;
use crate::util::prompt_bool;
use std::io::{self, Write};
use std::thread::sleep;
use std::time::Duration;

pub fn set_cpu(name: &str, n: u32) -> Result<()> {
    libvirt::require_defined(name)?;
    if n == 0 { return Err(Error::User("vcpus must be > 0".into())); }
    let ns = n.to_string();
    run("virsh", ["setvcpus", name, &ns, "--config", "--maximum"])?;
    run("virsh", ["setvcpus", name, &ns, "--config"])?;
    println!("vCPUs for '{name}' set to {n}. Reboot to apply.");
    Ok(())
}

pub fn set_ram(name: &str, gb: u32) -> Result<()> {
    libvirt::require_defined(name)?;
    if gb == 0 { return Err(Error::User("memory (GB) must be > 0".into())); }
    let mb = (gb as u64) * 1024;
    let s = format!("{mb}M");
    run("virsh", ["setmaxmem", name, &s, "--config"])?;
    run("virsh", ["setmem",    name, &s, "--config"])?;
    println!("RAM for '{name}' set to {gb}G. Reboot to apply.");
    Ok(())
}

pub fn resize_disk(cfg: &Config, name: &str, size: &str) -> Result<()> {
    libvirt::require_defined(name)?;
    crate::cmd::require("qemu-img")?;

    let disk = cfg.vm_disk(name);
    if !disk.exists() {
        return Err(Error::User(format!("disk file not found: {}", disk.display())));
    }

    if libvirt::is_running(name) {
        if !prompt_bool("VM must be off first. Shut down now?", false) {
            return Err(Error::User("aborted.".into()));
        }
        let _ = libvirt::shutdown(name);
        print!("Waiting for shutdown");
        io::stdout().flush().ok();
        let mut waited = 0;
        while libvirt::is_running(name) {
            print!(".");
            io::stdout().flush().ok();
            sleep(Duration::from_secs(2));
            waited += 2;
            if waited >= 120 {
                println!();
                return Err(Error::User(format!(
                    "'{name}' did not shut down within 120s. Try: qvm kill {name}"
                )));
            }
        }
        println!(" done.");
    }

    run("qemu-img", ["resize", disk.to_str().unwrap(), size])?;
    println!("Disk resized. cloud-init grows the root partition on next boot.");
    Ok(())
}
