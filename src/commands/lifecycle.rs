use crate::error::{Error, Result};
use crate::libvirt;
use crate::util::require_name;

pub fn start(name: &str)   -> Result<()> { do_action(name, libvirt::start) }
pub fn stop(name: &str)    -> Result<()> { do_action(name, libvirt::shutdown) }
pub fn restart(name: &str) -> Result<()> { do_action(name, libvirt::reboot) }
pub fn kill(name: &str)    -> Result<()> { do_action(name, libvirt::destroy) }

fn do_action(name: &str, f: fn(&str) -> Result<()>) -> Result<()> {
    libvirt::require_virsh()?;
    require_name(name)?;
    if !libvirt::exists(name) {
        return Err(Error::User(format!("VM '{name}' not found.")));
    }
    f(name)
}
