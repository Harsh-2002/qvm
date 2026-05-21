use crate::cmd::{require, run_inherit};
use crate::config::Config;
use crate::error::{Error, Result};
use std::fs;

pub fn run(cfg: &Config, distro: &str) -> Result<()> {
    require("wget")?;
    let d = cfg.distro(distro)?;
    let dest = cfg.image_path(distro)?;
    let tmp  = dest.with_extension("partial");

    println!("Pulling {distro}:");
    println!("  {}", d.url);
    println!("  -> {}", dest.display());

    let _ = fs::remove_file(&tmp);
    let r = run_inherit("wget", [
        "-q", "--show-progress",
        d.url.as_str(),
        "-O", tmp.to_str().unwrap(),
    ]);
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
        return Err(Error::User(format!(
            "download failed; {} is unchanged.", dest.display()
        )));
    }
    fs::rename(&tmp, &dest)?;
    println!("Ready: {}", dest.display());
    Ok(())
}
