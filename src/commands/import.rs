//! `qvm import <in.qvm.tar>` — restore a VM from an export tarball.
//!
//! The qcow2 disk and cloud-init seed move to qvm's managed dirs. The libvirt
//! domain is rebuilt from scratch via `virt-install --import` — never
//! replayed from the source host's `domain.xml`, because that XML contains
//! host-specific PCI paths that won't resolve here. Item 6 of the roadmap.

use crate::cmd;
use crate::commands::export::Meta;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Args {
    pub tarball: PathBuf,
    /// Optional rename on import.
    pub name:    Option<String>,
    /// Optional bridge override (default: cfg.network.bridge).
    pub bridge:  Option<String>,
    /// Optional osinfo override (default: meta.distro_hint resolved against
    /// the config's distro registry, falling back to "generic,require=off").
    pub osinfo:  Option<String>,
    /// Skip sha256 verification (use with caution).
    pub skip_verify: bool,
}

pub fn run(cfg: &Config, a: Args) -> Result<()> {
    use crate::style as s;
    cmd::require("tar")?;
    cmd::require("virt-install")?;
    cmd::require("qemu-img")?;

    if !a.tarball.exists() {
        return Err(Error::User(format!("not found: {}", a.tarball.display())));
    }
    cfg.ensure_dirs()?;

    // 1. Extract to a temp staging dir.
    let staging = std::env::temp_dir().join(format!("qvm-import-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    let _guard = StagingGuard(&staging);

    println!("{} extracting {}", s::label("import:"), a.tarball.display());
    cmd::run_inherit("tar", [
        "-xf", a.tarball.to_string_lossy().as_ref(),
        "-C",  staging.to_string_lossy().as_ref(),
    ])?;

    // 2. Read metadata.
    let meta_path = staging.join("qvm-meta.toml");
    if !meta_path.exists() {
        return Err(Error::User(
            "tarball is missing qvm-meta.toml — not a qvm export?".into(),
        ));
    }
    let meta_raw = fs::read_to_string(&meta_path)?;
    let meta = Meta::parse(&meta_raw);

    // 3. Cross-arch refusal.
    let target_arch = cmd::run("uname", ["-m"]).map(|s| s.trim().to_string())
        .unwrap_or_default();
    if !meta.arch.is_empty() && meta.arch != target_arch {
        return Err(Error::User(format!(
            "arch mismatch: tarball was exported for '{}', this host is '{}'.\n  \
             - cross-arch import is not supported; reinstall the guest OS on \
             this architecture instead.",
            meta.arch, target_arch
        )));
    }

    // 4. Resolve the new VM name.
    let new_name = match a.name.as_deref() {
        Some(n) => n.to_string(),
        None    => {
            if meta.name.is_empty() {
                return Err(Error::User(
                    "tarball has no name and --name was not supplied.".into()
                ));
            }
            meta.name.clone()
        }
    };
    crate::util::require_name(&new_name)?;
    if libvirt::exists(&new_name) {
        return Err(Error::User(format!(
            "VM '{new_name}' already exists. Pass --name <newname> to import alongside it."
        )));
    }

    // 5. Verify disk presence + (optional) sha256.
    let staged_disk = staging.join("disk.qcow2");
    if !staged_disk.exists() {
        return Err(Error::User("tarball is missing disk.qcow2.".into()));
    }
    if !a.skip_verify {
        if let Some(expected) = meta.disk_sha256.as_deref() {
            print!("{} verifying disk sha256… ", s::label("verify:"));
            let actual = sha256_file(&staged_disk)?;
            if actual != expected {
                println!("{}", s::err("MISMATCH"));
                return Err(Error::User(format!(
                    "disk.qcow2 sha256 mismatch:\n  expected {}\n  actual   {}\n\
                     The tarball is corrupted or was truncated in transit.",
                    expected, actual,
                )));
            }
            println!("{}", s::ok("ok"));
        }
    }
    // Sanity-check: must be self-contained. (Defensive — convert wrote it.)
    let info = cmd::run("qemu-img", ["info", staged_disk.to_string_lossy().as_ref()])?;
    if info.lines().any(|l| l.trim_start().starts_with("backing file:")) {
        return Err(Error::User(
            "tarball disk.qcow2 has a backing file — refusing to import a non-self-contained image.".into()
        ));
    }

    // 6. Move staged files into qvm's managed dirs.
    let final_disk = cfg.vm_disk(&new_name);
    let final_iso  = cfg.vm_seed_iso(&new_name);
    let final_ci   = cfg.vm_ci_dir(&new_name);

    move_or_copy(&staged_disk, &final_disk)?;
    let staged_iso = staging.join("cloud-init.iso");
    if staged_iso.exists() {
        move_or_copy(&staged_iso, &final_iso)?;
    }
    let staged_vmu = staging.join(".vmuser");
    if staged_vmu.exists() {
        fs::create_dir_all(&final_ci)?;
        move_or_copy(&staged_vmu, &final_ci.join(".vmuser"))?;
    }

    // 7. Rebuild the libvirt domain via virt-install --import. We do NOT
    //    replay the source's domain.xml — its PCI paths and bridge name
    //    won't resolve on this host.
    let cpus = meta.cpus.unwrap_or(cfg.defaults.cpus);
    let mem_gb = meta.memory_gb.unwrap_or(cfg.defaults.memory_gb);
    let bridge = a.bridge.unwrap_or_else(|| cfg.network.bridge.clone());

    // osinfo: prefer the registered distro's value, else "generic".
    let osinfo = a.osinfo.unwrap_or_else(|| {
        if let Some(d) = cfg.distros.get(&meta.distro_hint) {
            format!("name={},require=off", d.osinfo)
        } else {
            "name=generic,require=off".into()
        }
    });

    let memory_mb = (mem_gb as u64) * 1024;
    let netarg    = format!("bridge={},model=virtio", bridge);
    let diskarg   = format!("path={},format=qcow2,bus=virtio", final_disk.display());
    let vncarg    = format!("vnc,listen={}", cfg.vnc.bind);

    let mut args: Vec<String> = vec![
        "--name".into(),       new_name.clone(),
        "--memory".into(),     memory_mb.to_string(),
        "--vcpus".into(),      cpus.to_string(),
        // host-model is more portable than host-passthrough across hardware.
        "--cpu".into(),        "host-model".into(),
        "--disk".into(),       diskarg,
        "--osinfo".into(),     osinfo,
        "--graphics".into(),   vncarg,
        "--network".into(),    netarg,
        "--channel".into(),    "unix,target_type=virtio,name=org.qemu.guest_agent.0".into(),
        "--memballoon".into(), "model=virtio".into(),
        "--import".into(),
        "--noautoconsole".into(),
    ];
    if final_iso.exists() {
        let cdromarg = format!("path={},device=cdrom", final_iso.display());
        args.push("--disk".into()); args.push(cdromarg);
    }
    // UEFI guess: if the registered distro is UEFI-only, replay that.
    if let Some(d) = cfg.distros.get(&meta.distro_hint) {
        if d.uefi {
            args.push("--machine".into()); args.push("q35".into());
            args.push("--boot".into());    args.push("uefi,loader.secure=no".into());
        }
    }

    println!("{} defining + starting '{}'", s::label("virt-install:"), new_name);
    cmd::run_inherit("virt-install", args.iter().map(|s| s.as_str()))?;

    if cfg.defaults.autostart {
        let _ = libvirt::autostart_on(&new_name);
    }
    println!("{} imported as '{}'", s::ok("✓"), new_name);
    Ok(())
}

fn move_or_copy(src: &Path, dst: &Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    // Try rename first (same filesystem). On cross-filesystem moves
    // (most common: /tmp → /var/lib/qvm/vms), fall back to copy + remove.
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(src, dst)?;
            let _ = fs::remove_file(src);
            Ok(())
        }
    }
}

fn sha256_file(p: &Path) -> Result<String> {
    let mut f = fs::File::open(p)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    let bytes = hasher.finalize();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes { s.push_str(&format!("{:02x}", b)); }
    Ok(s)
}

struct StagingGuard<'a>(&'a Path);
impl Drop for StagingGuard<'_> {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.0);
    }
}
