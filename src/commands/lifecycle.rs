use crate::error::Result;
use crate::libvirt;

pub fn start(name: &str)   -> Result<()> { do_action(name, libvirt::start) }
pub fn stop(name: &str)    -> Result<()> { do_action(name, libvirt::shutdown) }
pub fn restart(name: &str) -> Result<()> { do_action(name, libvirt::reboot) }
pub fn kill(name: &str)    -> Result<()> { do_action(name, libvirt::destroy) }

fn do_action(name: &str, f: fn(&str) -> Result<()>) -> Result<()> {
    libvirt::require_defined(name)?;
    f(name)
}
