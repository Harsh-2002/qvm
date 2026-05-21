use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt;
use crate::util::confirm_phrase;
use std::fs;

pub fn run(cfg: &Config, name: &str, force: bool) -> Result<()> {
    libvirt::require_defined(name)?;

    if !force && !confirm_phrase(
        &format!("Delete '{name}' and all its data? Type 'yes':"),
        "yes",
    ) {
        println!("Cancelled.");
        return Ok(());
    }

    // 1. Force-off if running. We don't fail if destroy errors — undefine
    //    handles a stopped domain too, and if libvirt thinks it's stopped
    //    when it isn't, undefine will surface the real error below.
    if libvirt::is_running(name) {
        let _ = libvirt::destroy(name);
    }

    // 2. Undefine — propagate any error. We must NOT remove disks if the
    //    libvirt domain is still defined; that would leave the VM wedged.
    libvirt::undefine(name).map_err(|e| Error::User(format!(
        "failed to undefine '{name}': {e}\n\
         Disk files have NOT been removed. Run manually:\n  \
         virsh undefine {name} --nvram --remove-all-storage"
    )))?;

    // 3. Now safe to remove our managed files.
    let _ = fs::remove_file(cfg.vm_disk(name));
    let _ = fs::remove_file(cfg.vm_seed_iso(name));
    let _ = fs::remove_dir_all(cfg.vm_ci_dir(name));

    println!("Deleted '{name}'.");
    Ok(())
}
