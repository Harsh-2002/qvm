use crate::error::{Error, Result};
use crate::libvirt;

#[derive(Debug, Clone, Copy)]
pub enum Verb { Start, Stop, Restart, Kill }
impl Verb {
    fn label(self) -> &'static str {
        match self { Verb::Start => "start", Verb::Stop => "stop",
                     Verb::Restart => "restart", Verb::Kill => "kill" }
    }
    fn action(self) -> fn(&str) -> Result<()> {
        match self {
            Verb::Start   => libvirt::start,
            Verb::Stop    => libvirt::shutdown,
            Verb::Restart => libvirt::reboot,
            Verb::Kill    => libvirt::destroy,
        }
    }
}

pub fn start(name: &str)   -> Result<()> { single(Verb::Start,   name) }
pub fn stop(name: &str)    -> Result<()> { single(Verb::Stop,    name) }
pub fn restart(name: &str) -> Result<()> { single(Verb::Restart, name) }
pub fn kill(name: &str)    -> Result<()> { single(Verb::Kill,    name) }

fn single(v: Verb, name: &str) -> Result<()> {
    libvirt::require_defined(name)?;
    v.action()(name)
}

/// Batch dispatch for `qvm start a b c`. Prints `✓ name` / `✗ name (err)`
/// per row, returns Err if any failed (caller maps to exit code 1).
/// Never short-circuits — every name gets a try.
pub fn batch(v: Verb, names: &[String], all: bool) -> Result<()> {
    use crate::style as s;
    let targets: Vec<String> = if all {
        // For start --all we operate on every defined VM; for stop/kill
        // it's the *running* set (no-op on already-stopped VMs).
        let doms = libvirt::domains()?;
        match v {
            Verb::Start => doms.into_iter()
                .filter(|d| d.state != "running")
                .map(|d| d.name).collect(),
            _           => doms.into_iter()
                .filter(|d| d.state == "running")
                .map(|d| d.name).collect(),
        }
    } else {
        names.to_vec()
    };

    if targets.is_empty() {
        println!("{} no matching VMs to {}.", s::dim("·"), v.label());
        return Ok(());
    }

    let mut failed: Vec<String> = Vec::new();
    for name in &targets {
        match single(v, name) {
            Ok(())  => println!("  {} {}", s::ok("✓"), name),
            Err(e)  => {
                println!("  {} {}  {}", s::err("✗"), name, s::dim(format!("({e})")));
                failed.push(name.clone());
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Error::User(format!(
            "{} of {} {} operation(s) failed: {}",
            failed.len(), targets.len(), v.label(), failed.join(", ")
        )))
    }
}
