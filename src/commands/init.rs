use crate::cmd::run_inherit;
use crate::config::{self, Config};
use crate::error::Result;
use std::fs;
use std::path::Path;

pub fn run(config_path: &Path, pull_all: bool) -> Result<()> {
    // 1. Write the sample config if missing.
    if !config_path.exists() {
        if let Some(parent) = config_path.parent() { fs::create_dir_all(parent)?; }
        fs::write(config_path, config::sample_toml())?;
        println!("Wrote {}", config_path.display());
    } else {
        println!("Config already exists at {} (leaving it alone).", config_path.display());
    }

    // 2. Ensure data directories exist.
    let cfg = Config::load(Some(config_path))?;
    cfg.ensure_dirs()?;
    println!("Prepared:");
    println!("  images   : {}", cfg.paths.images.display());
    println!("  vms      : {}", cfg.paths.vms.display());
    println!("  cloudinit: {}", cfg.paths.cloudinit.display());

    // 3. Optionally pre-pull all baseline images.
    if pull_all {
        println!("\nDownloading baseline images ({} distros)...", cfg.distros.len());
        for (key, d) in &cfg.distros {
            let dest = cfg.paths.images.join(&d.image);
            if dest.exists() {
                println!("  [skip] {key:14}  already present");
                continue;
            }
            println!("  [pull] {key:14}  {}", d.url);
            let tmp = dest.with_extension("partial");
            let _ = fs::remove_file(&tmp);
            // wget -q --show-progress
            let r = run_inherit("wget", [
                "-q", "--show-progress",
                d.url.as_str(),
                "-O", tmp.to_str().unwrap(),
            ]);
            match r {
                Ok(_) => { fs::rename(&tmp, &dest)?; }
                Err(e) => {
                    let _ = fs::remove_file(&tmp);
                    eprintln!("  [warn] failed: {e}");
                }
            }
        }
    } else {
        println!("\nNext steps:");
        println!("  qvm pull debian:13       # download a single distro");
        println!("  qvm init --pull-all      # download all baseline distros");
        println!("  qvm run myvm debian:13   # create your first VM");
    }
    Ok(())
}
