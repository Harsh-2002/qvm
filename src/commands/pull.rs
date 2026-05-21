use crate::cmd::{require, run_inherit};
use crate::config::Config;
use crate::error::{Error, Result};
use std::fs;

pub fn run(cfg: &Config, distro: &str) -> Result<()> {
    require("wget")?;
    let d = cfg.distro(distro)?;
    println!("Pulling {distro}:");
    println!("  {}", d.url);
    println!("  -> {}", cfg.image_path(distro)?.display());
    pull_one(cfg, distro)?;
    println!("Ready: {}", cfg.image_path(distro)?.display());
    Ok(())
}

/// Atomic download for a single distro: wget to `<image>.partial`, rename on success.
///
/// Used by both `qvm pull <distro>` and `qvm init --pull-all`. Caller is
/// expected to have already verified that `wget` is on PATH.
pub fn pull_one(cfg: &Config, distro: &str) -> Result<()> {
    let d    = cfg.distro(distro)?;
    let dest = cfg.image_path(distro)?;
    let tmp  = dest.with_extension("partial");

    let _ = fs::remove_file(&tmp);
    let r = run_inherit("wget", [
        "-q", "--show-progress",
        d.url.as_str(),
        "-O", tmp.to_str().unwrap(),
    ]);
    if let Err(e) = r {
        let _ = fs::remove_file(&tmp);
        return Err(Error::User(format!(
            "download failed for {distro}: {e}; {} is unchanged.",
            dest.display()
        )));
    }
    fs::rename(&tmp, &dest)?;
    Ok(())
}
