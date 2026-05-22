//! `qvm clone <src> <dst>` — duplicate an existing VM.
//!
//! Three modes (mirrors `qvm export`):
//!   - **Live** (`--live`): take a `--quiesce` disk-only snapshot of the
//!     running source, convert the now-read-only base into the clone's
//!     disk, then `blockcommit --active --pivot` to merge the overlay
//!     back. Crash-consistent, zero downtime. Requires a responsive
//!     qemu-guest-agent in the source.
//!   - **Stop** (`--stop`): if the source is running, shut it down
//!     cleanly, convert, then restart it. Brief downtime on the source.
//!   - **Auto** (default): live if source is running and the guest
//!     agent answers; offline if source is stopped; **error** if the
//!     source is running without an agent — caller must opt in to
//!     either `--live` (fix the agent) or `--stop` (allow downtime).
//!
//! Mechanics common to all modes:
//!   1. Resolve source's cpus/memory/disk from `virsh dominfo` + `qemu-img info`.
//!   2. Recover the source's login user and password hash from the
//!      cloud-init seed on disk (so the clone boots with the same
//!      credentials the operator already knows).
//!   3. `qemu-img convert -O qcow2 src dst` — FULL copy, no backing file
//!      (invariant 3.1). Optionally `qemu-img resize` if `--disk` was
//!      specified larger than the source.
//!   4. Build a fresh cloud-init seed for the destination with a new
//!      `instance-id` (= dst name). cloud-init detects the changed
//!      instance-id on boot and re-runs per-instance modules — so the
//!      clone picks up the new hostname, generates fresh SSH host keys,
//!      and refreshes its machine-id without any in-guest intervention.
//!   5. `virt-install --import` for the new domain. Detects UEFI from
//!      the source's domain XML so the clone matches the source's
//!      firmware path.

use crate::cloudinit::{login_user_of, Seed};
use crate::cmd::{require, run as cmd_run, run_inherit};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt;
use crate::util::require_username;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Auto-detect: live if running + agent, offline if stopped,
    /// error if running without agent.
    Auto,
    /// Force live; fail if not running or no guest agent.
    Live,
    /// Force offline; stop the running source then restart afterward.
    Stop,
}

#[derive(Debug)]
pub struct Args {
    pub src:       String,
    pub dst:       String,
    pub mode:      Mode,
    pub cpus:      Option<u32>,
    pub memory_gb: Option<u32>,
    /// New disk size in GB. Must be >= source's disk; smaller errors out.
    pub disk_gb:   Option<u32>,
    pub no_autostart: bool,
    /// `Some(false)` forces nested virt off, `None` inherits cfg.defaults.
    pub nested:    Option<bool>,
}

enum Resolved { Live, Offline }

pub fn run(cfg: &Config, a: Args) -> Result<()> {
    libvirt::require_defined(&a.src)?;
    libvirt::require_absent(&a.dst)?;
    require("virt-install")?;
    require("qemu-img")?;
    require("genisoimage")?;

    let was_running = libvirt::is_running(&a.src);

    let resolved = match a.mode {
        Mode::Stop => Resolved::Offline,
        Mode::Live => {
            if !was_running {
                return Err(Error::User(format!(
                    "--live requires '{}' to be running. Start it first or drop --live.",
                    a.src
                )));
            }
            super::export::require_guest_agent(&a.src)?;
            Resolved::Live
        }
        Mode::Auto => {
            if !was_running {
                Resolved::Offline
            } else if super::export::guest_agent_alive(&a.src) {
                Resolved::Live
            } else {
                return Err(Error::User(format!(
                    "source '{src}' is running but qemu-guest-agent is not responsive.\n  \
                     - install/start qemu-guest-agent in the guest, then retry, or\n  \
                     - pass --stop to allow a brief downtime (stop + clone + restart), or\n  \
                     - stop it manually first: qvm stop {src}",
                    src = a.src,
                )));
            }
        }
    };

    // ── recover source's parameters ──────────────────────────────────────────
    let src_disk = cfg.vm_disk(&a.src);
    if !src_disk.exists() {
        return Err(Error::User(format!(
            "source disk not found at {}. Was '{}' created by qvm?",
            src_disk.display(), a.src,
        )));
    }
    let src_disk_gb = super::export::qemu_img_virtual_size_gb(&src_disk)
        .ok_or_else(|| Error::User(format!(
            "could not read source disk size at {}", src_disk.display()
        )))?;
    let cpus   = a.cpus.unwrap_or_else(||
        super::export::dominfo_cpus(&a.src).unwrap_or(cfg.defaults.cpus));
    let ram_gb = a.memory_gb.unwrap_or_else(||
        super::export::dominfo_memory_gb(&a.src).unwrap_or(cfg.defaults.memory_gb));
    let disk_gb = match a.disk_gb {
        Some(n) if n < src_disk_gb => return Err(Error::User(format!(
            "--disk {n}G is smaller than source ({src_disk_gb}G). qcow2 can't \
             safely shrink without in-guest cooperation."
        ))),
        Some(n) => n,
        None    => src_disk_gb,
    };

    // login user from sidecar
    let user = login_user_of(cfg, &a.src).ok_or_else(|| Error::User(format!(
        "no .vmuser sidecar for '{}' — clone needs the source's login user. \
         Was the source created by qvm?", a.src
    )))?;
    require_username(&user)?;

    // password hash + shell from source user-data
    let src_userdata = cfg.vm_ci_dir(&a.src).join("user-data");
    if !src_userdata.exists() {
        return Err(Error::User(format!(
            "source user-data missing at {}. Cannot recover credentials for \
             clone; re-create '{}' instead.",
            src_userdata.display(), a.src,
        )));
    }
    let src_ud = std::fs::read_to_string(&src_userdata)?;
    let pw_hash = extract_passwd_hash(&src_ud).ok_or_else(|| Error::User(format!(
        "could not find passwd hash in {}. Source's cloud-init seed is missing \
         the user's password hash.", src_userdata.display(),
    )))?;
    let shell = extract_shell(&src_ud).unwrap_or_else(|| "/bin/bash".to_string());

    // UEFI: detect by checking source's domain XML for an <nvram> element,
    // which is only present on UEFI-booted domains.
    let src_xml = cmd_run("virsh", ["dumpxml", &a.src])?;
    let uefi = src_xml.contains("<nvram") || src_xml.contains("OVMF") || crate::arch::is_arm();

    // osinfo: try to pull `libosinfo:os id` from the source XML; default to
    // generic linux. require=off keeps virt-install non-fatal on misses.
    let osinfo = extract_osinfo(&src_xml).unwrap_or_else(|| "linux2022".to_string());

    cfg.ensure_dirs()?;
    let dst_disk = cfg.vm_disk(&a.dst);
    let dst_iso  = cfg.vm_seed_iso(&a.dst);
    let dst_ci   = cfg.vm_ci_dir(&a.dst);

    // ── copy disk: FULL convert, no backing file ─────────────────────────────
    use crate::style as s;
    let mode_label = match resolved { Resolved::Live => "live", Resolved::Offline => "offline" };
    println!("{} cloning '{}' → '{}' ({src_disk_gb}G disk, {mode_label})",
        s::label("clone:"), a.src, a.dst);

    match resolved {
        Resolved::Live => {
            live_clone_disk(&a.src, &src_disk, &dst_disk)?;
        }
        Resolved::Offline => {
            if was_running {
                println!("{} stopping '{}' for clean clone...", s::label("clone:"), a.src);
                super::export::stop_and_wait(&a.src)?;
            }
            run_inherit("qemu-img", [
                "convert", "-p", "-O", "qcow2",
                src_disk.to_str().unwrap(),
                dst_disk.to_str().unwrap(),
            ])?;
        }
    }

    if disk_gb > src_disk_gb {
        cmd_run("qemu-img", [
            "resize", "-q",
            dst_disk.to_str().unwrap(),
            &format!("{disk_gb}G"),
        ])?;
    }

    // ── fresh cloud-init seed (new instance-id triggers re-run) ──────────────
    println!("{} generating cloud-init seed for '{}'...", s::label("clone:"), a.dst);
    Seed {
        vm_name: &a.dst,
        login_user: &user,
        login_shell: &shell,
        password_hash: &pw_hash,
        ssh_keys: &cfg.ssh_keys,
        grub_timeout: cfg.defaults.grub_timeout,
    }.build(&dst_ci, &dst_iso)?;

    // ── define + start ──────────────────────────────────────────────────────
    println!("{} defining and starting '{}'...", s::label("clone:"), a.dst);
    let memory_mb = (ram_gb as u64) * 1024;
    let cpus_str   = cpus.to_string();
    let memory_str = memory_mb.to_string();
    let osinfo_arg = format!("name={},require=off", osinfo);
    let netarg     = format!("bridge={},model=virtio", cfg.network.bridge);
    let diskarg    = format!("path={},format=qcow2,bus=virtio", dst_disk.display());
    let cdromarg   = format!("path={},device=cdrom", dst_iso.display());
    let vncarg     = format!("vnc,listen={}", cfg.vnc.bind);

    let nested = a.nested.unwrap_or(cfg.defaults.nested);
    let cpu_arg: String = if nested { "host-passthrough".into() }
                          else      { "host-model,-vmx,-svm".into() };

    let mut args: Vec<String> = vec![
        "--name".into(),       a.dst.clone(),
        "--memory".into(),     memory_str,
        "--vcpus".into(),      cpus_str,
        "--cpu".into(),        cpu_arg,
        "--disk".into(),       diskarg,
        "--disk".into(),       cdromarg,
        "--osinfo".into(),     osinfo_arg,
        "--graphics".into(),   vncarg,
        "--network".into(),    netarg,
        "--channel".into(),    "unix,target_type=virtio,name=org.qemu.guest_agent.0".into(),
        "--memballoon".into(), "model=virtio".into(),
        "--import".into(),
        "--noautoconsole".into(),
    ];
    if crate::arch::is_arm() {
        args.push("--arch".into());    args.push("aarch64".into());
        args.push("--machine".into()); args.push("virt".into());
        args.push("--boot".into());    args.push("uefi,loader.secure=no".into());
    } else if uefi {
        args.push("--machine".into()); args.push("q35".into());
        args.push("--boot".into());    args.push("uefi,loader.secure=no".into());
    }
    run_inherit("virt-install", args.iter().map(|s| s.as_str()))?;

    let autostart = !a.no_autostart && cfg.defaults.autostart;
    if autostart { libvirt::autostart_on(&a.dst)?; }

    // ── restart the source if we stopped it for an offline clone ────────────
    if matches!(resolved, Resolved::Offline) && was_running {
        println!("{} restarting source '{}'", s::label("clone:"), a.src);
        let _ = libvirt::start(&a.src);
    }

    // ── summary ─────────────────────────────────────────────────────────────
    println!();
    println!("{} {}",
        s::ok("✓"), s::ok(format!("VM '{}' cloned from '{}' ({mode_label})", a.dst, a.src)));
    println!(
        "  {} {cpus}   {} {ram_gb}G   {} {disk_gb}G   {} {user}",
        s::label("cpus"), s::label("ram"), s::label("disk"), s::label("user"),
    );
    println!();
    println!("  {} {dst}        {}", s::cmd("qvm ip"),      s::dim("# wait ~30s for cloud-init to re-run"), dst = a.dst);
    println!("  {} {dst}   {}",      s::cmd("qvm ssh-cmd"), s::dim("# same login as source"), dst = a.dst);
    Ok(())
}

// ── live mode disk copy (AWS-EBS-style snapshot + blockcommit) ───────────────
//
// Steps, in order, with the same error-recovery posture as
// `export::live_export`:
//
//   1. `virsh snapshot-create-as --disk-only --quiesce` on the source.
//      An overlay file captures writes that arrive during the clone;
//      the original disk freezes read-only.
//   2. `qemu-img convert` the frozen original into the destination disk.
//      No backing file (invariant 3.1).
//   3. `virsh blockcommit --active --pivot` to merge the overlay back
//      into the source's original disk so the source is no longer
//      running on an overlay.
//
// If step 2 fails, we abort any in-progress block job, drop the overlay
// and the partial dst, and surface a clear error pointing at --stop.
// If step 3 fails, the source is left on the overlay; we surface a
// LOUD error with manual recovery instructions rather than silently
// pretending everything's fine — same policy as export.
fn live_clone_disk(src_name: &str, src_disk: &Path, dst_disk: &Path) -> Result<()> {
    let overlay = src_disk.with_extension("clone-overlay.qcow2");
    let _ = std::fs::remove_file(&overlay);

    let diskspec = format!("vda,file={}", overlay.display());
    cmd_run("virsh", [
        "snapshot-create-as", src_name, "_qvm_clone",
        "--disk-only", "--quiesce", "--no-metadata",
        "--diskspec", &diskspec,
    ]).map_err(|e| Error::User(format!(
        "live snapshot failed: {e}\n  - rerun with --stop for the offline path."
    )))?;

    let convert_res = run_inherit("qemu-img", [
        "convert", "-p", "-O", "qcow2",
        src_disk.to_str().unwrap(),
        dst_disk.to_str().unwrap(),
    ]);

    // Always attempt pivot — leaving the source on the overlay is worse
    // than failing the clone.
    let pivot_res = run_inherit("virsh", [
        "blockcommit", src_name, "vda",
        "--active", "--pivot",
        "--base", src_disk.to_str().unwrap(),
        "--top",  overlay.to_str().unwrap(),
    ]);

    if convert_res.is_err() {
        let _ = cmd_run("virsh", ["blockjob", src_name, "vda", "--abort"]);
        let _ = std::fs::remove_file(&overlay);
        let _ = std::fs::remove_file(dst_disk);
        return Err(Error::User(
            "live clone: disk convert failed mid-snapshot. \
             Source has been restored. Rerun with --stop for a clean offline clone.".into()
        ));
    }
    if pivot_res.is_err() {
        return Err(Error::User(format!(
            "live clone: snapshot was taken and clone disk was written, but \
             blockcommit --pivot failed. The SOURCE '{src_name}' is RUNNING ON \
             THE OVERLAY at {}. Clone disk is at {}. Recover the source with:\n  \
             virsh blockjob {src_name} vda --abort\n  \
             qvm stop {src_name} ; qvm start {src_name}\n\
             Or merge manually: virsh blockcommit {src_name} vda --active --pivot.",
             overlay.display(), dst_disk.display(),
        )));
    }
    let _ = std::fs::remove_file(&overlay);
    Ok(())
}

/// Pull `passwd: "$6$..."` out of cloud-init user-data.
pub fn extract_passwd_hash(user_data: &str) -> Option<String> {
    for line in user_data.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("passwd:") {
            let rest = rest.trim();
            // Strip surrounding quotes if any.
            let unquoted = rest.trim_start_matches('"').trim_end_matches('"');
            if !unquoted.is_empty() {
                return Some(unquoted.to_string());
            }
        }
    }
    None
}

/// Pull `shell: /bin/...` out of cloud-init user-data (first match).
pub fn extract_shell(user_data: &str) -> Option<String> {
    for line in user_data.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("shell:") {
            let s = rest.trim().to_string();
            if !s.is_empty() { return Some(s); }
        }
    }
    None
}

/// Extract `<libosinfo:os id="http://.../debian/13"/>` short id from
/// `virsh dumpxml`. Returns just the last path segment (e.g. "debian13"
/// → mapped to "debian13" or whatever the URI has). Best-effort; falls
/// back to None so the caller substitutes a generic default.
pub fn extract_osinfo(xml: &str) -> Option<String> {
    let pat = "libosinfo:os id=\"";
    let start = xml.find(pat)? + pat.len();
    let end = xml[start..].find('"')?;
    let uri = &xml[start..start + end];
    // URI looks like http://debian.org/debian/13 or http://fedoraproject.org/fedora/41
    let segments: Vec<&str> = uri.rsplit('/').collect();
    if segments.len() >= 2 {
        // Two segments back from end: distro name; one back: version.
        let ver = segments[0];
        let distro = segments[1];
        Some(format!("{distro}{ver}"))
    } else {
        None
    }
}
