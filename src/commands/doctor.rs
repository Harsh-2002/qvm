//! `qvm doctor` - check host dependencies and optionally install them.

use crate::cmd::{have, run};
use crate::error::{Error, Result};
use std::fs;
use std::io::{self, Write};

/// Every external program qvm depends on, plus what package provides it
/// on each distro family. The first entry per row is the canonical name
/// (apt/Debian-style); the rest are family overrides.
struct Dep {
    /// Binary qvm calls.
    binary: &'static str,
    /// Why qvm needs it (shown to the user).
    why: &'static str,
    /// Package name on each distro family.
    /// (debian/ubuntu, fedora/rhel, alpine, arch)
    packages: Packages,
}

struct Packages {
    apt:    &'static str,  // debian, ubuntu
    dnf:    &'static str,  // fedora, rocky, alma, rhel
    apk:    &'static str,  // alpine
    pacman: &'static str,  // arch
}

const DEPS: &[Dep] = &[
    Dep {
        binary: "virsh",
        why: "manage libvirt domains (start, stop, list, undefine)",
        packages: Packages {
            apt:    "libvirt-clients libvirt-daemon-system",
            dnf:    "libvirt-client libvirt-daemon",
            apk:    "libvirt-client libvirt-daemon",
            pacman: "libvirt",
        },
    },
    Dep {
        binary: "virt-install",
        why: "define and start new VMs (one-shot import flow)",
        packages: Packages {
            apt:    "virtinst",
            dnf:    "virt-install",
            apk:    "virt-install",
            pacman: "virt-install",
        },
    },
    Dep {
        binary: "qemu-img",
        why: "create and resize per-VM qcow2 disks (full-copy, no overlays)",
        packages: Packages {
            apt:    "qemu-utils",
            dnf:    "qemu-img",
            apk:    "qemu-img",
            pacman: "qemu-img",
        },
    },
    Dep {
        binary: "qemu-system-x86_64",
        why: "the actual VM emulator (KVM acceleration on amd64)",
        packages: Packages {
            apt:    "qemu-system-x86",
            dnf:    "qemu-kvm",
            apk:    "qemu-system-x86_64",
            pacman: "qemu-system-x86",
        },
    },
    Dep {
        binary: "genisoimage",
        why: "build the cloud-init seed ISO (NoCloud datasource)",
        packages: Packages {
            apt:    "genisoimage",
            dnf:    "genisoimage",
            apk:    "cdrkit",
            pacman: "cdrkit",
        },
    },
    Dep {
        binary: "wget",
        why: "download distro base images (atomic, with progress)",
        packages: Packages {
            apt:    "wget",
            dnf:    "wget",
            apk:    "wget",
            pacman: "wget",
        },
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Debian,    // debian, ubuntu, mint, ...
    Fedora,    // fedora, rocky, alma, rhel, centos
    Alpine,
    Arch,
    Unknown,
}

impl Family {
    fn from_os_release() -> Self {
        let raw = match fs::read_to_string("/etc/os-release") {
            Ok(s) => s,
            Err(_) => return Family::Unknown,
        };
        // Check ID_LIKE first (most general), then ID.
        let mut id = String::new();
        let mut id_like = String::new();
        for line in raw.lines() {
            if let Some(v) = line.strip_prefix("ID=") {
                id = v.trim_matches('"').to_ascii_lowercase();
            } else if let Some(v) = line.strip_prefix("ID_LIKE=") {
                id_like = v.trim_matches('"').to_ascii_lowercase();
            }
        }
        let blob = format!("{id} {id_like}");
        if blob.contains("debian") || blob.contains("ubuntu") { Family::Debian }
        else if blob.contains("fedora") || blob.contains("rhel")
             || blob.contains("rocky") || blob.contains("centos") { Family::Fedora }
        else if blob.contains("alpine") { Family::Alpine }
        else if blob.contains("arch")   { Family::Arch }
        else { Family::Unknown }
    }

    fn install_cmd(&self) -> Option<&'static str> {
        match self {
            Family::Debian => Some("apt-get install -y"),
            Family::Fedora => Some("dnf install -y"),
            Family::Alpine => Some("apk add"),
            Family::Arch   => Some("pacman -S --noconfirm"),
            Family::Unknown => None,
        }
    }

    fn packages<'a>(&self, p: &'a Packages) -> &'a str {
        match self {
            Family::Debian => p.apt,
            Family::Fedora => p.dnf,
            Family::Alpine => p.apk,
            Family::Arch   => p.pacman,
            Family::Unknown=> p.apt,  // fallback to most common
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Family::Debian => "Debian/Ubuntu",
            Family::Fedora => "Fedora/RHEL/Rocky/Alma",
            Family::Alpine => "Alpine",
            Family::Arch   => "Arch",
            Family::Unknown=> "unknown distro",
        }
    }
}

/// Check libvirt daemon is reachable. Most distros need `systemctl enable
/// --now libvirtd`. We don't enable it ourselves (root-level service
/// management is the user's call), but we report it.
fn libvirtd_ok() -> bool {
    run("virsh", ["-c", "qemu:///system", "list"]).is_ok()
}

/// Check that the user is root (we should be, since main.rs enforces it,
/// but we re-check defensively for "qvm doctor" specifically).
fn is_root() -> bool {
    unsafe { libc_geteuid() == 0 }
}

extern "C" { fn geteuid() -> u32; }
unsafe fn libc_geteuid() -> u32 { unsafe { geteuid() } }

pub fn run_doctor(install: bool) -> Result<()> {
    let family = Family::from_os_release();
    println!("Host: {}", family.name());
    println!();

    // 1. Binary presence check
    println!("Dependencies:");
    let mut missing: Vec<&Dep> = Vec::new();
    for d in DEPS {
        if have(d.binary) {
            println!("  [ok]   {:<22} {}", d.binary, d.why);
        } else {
            println!("  [MISS] {:<22} {}", d.binary, d.why);
            missing.push(d);
        }
    }

    // 2. libvirtd reachability (only meaningful if virsh is present)
    println!();
    println!("Services:");
    if have("virsh") {
        if libvirtd_ok() {
            println!("  [ok]   libvirtd reachable");
        } else {
            println!("  [WARN] libvirtd not reachable. Enable with:");
            println!("           systemctl enable --now libvirtd");
        }
    } else {
        println!("  [skip] libvirtd check (virsh missing)");
    }

    // 3. Root check
    println!();
    if is_root() {
        println!("  [ok]   running as root");
    } else {
        println!("  [WARN] qvm must be run as root for VM operations");
    }

    if missing.is_empty() {
        println!();
        println!("All required dependencies are installed.");
        print_examples();
        return Ok(());
    }

    // 4. Install hint or actual install
    println!();
    println!("Missing {} package(s).", missing.len());

    let install_cmd = match family.install_cmd() {
        Some(c) => c,
        None => {
            println!("Unknown distro - install these yourself:");
            for d in &missing {
                println!("  {}: provided by package similar to '{}'", d.binary, d.packages.apt);
            }
            return Err(Error::User("dependencies missing; see above".into()));
        }
    };

    // Compose one install line covering all missing packages, deduped.
    let mut pkgs: Vec<&str> = Vec::new();
    for d in &missing {
        for p in family.packages(&d.packages).split_whitespace() {
            if !pkgs.contains(&p) { pkgs.push(p); }
        }
    }
    let full = format!("{install_cmd} {}", pkgs.join(" "));

    if !install {
        println!();
        println!("Run this to install everything:");
        println!("  {full}");
        println!();
        println!("Or let qvm do it: `qvm doctor --install`");
        return Err(Error::User("dependencies missing".into()));
    }

    // --install: confirm, then run
    if !is_root() {
        return Err(Error::User("--install requires root".into()));
    }
    println!();
    println!("About to run:");
    println!("  {full}");
    print!("Proceed? [y/N]: ");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    if !line.trim().eq_ignore_ascii_case("y") {
        return Err(Error::User("aborted".into()));
    }

    // Most package managers refresh metadata implicitly, but apt is the
    // common exception worth handling.
    if family == Family::Debian {
        let _ = crate::cmd::run_inherit("apt-get", ["update"]);
    }

    // Run the install. We split the install_cmd into its parts.
    let mut parts = install_cmd.split_whitespace();
    let prog = parts.next().unwrap();
    let cmd_args: Vec<&str> = parts.collect();
    let mut final_args: Vec<&str> = cmd_args;
    for p in &pkgs { final_args.push(p); }

    crate::cmd::run_inherit(prog, final_args)?;

    println!();
    println!("Re-running checks...");
    run_doctor(false)
}

fn print_examples() {
    println!();
    println!("Quick-start examples:");
    println!();
    println!("  # First-run setup (interactive wizard)");
    println!("  qvm init");
    println!();
    println!("  # First-run setup, non-interactive, download all base images");
    println!("  qvm init --yes --pull-all");
    println!();
    println!("  # Create a VM (uses config defaults for CPU/RAM/disk)");
    println!("  qvm run web01 debian:13");
    println!();
    println!("  # Create with explicit resources and a known username");
    println!("  qvm run db01 ubuntu:24.04 -c 4 -m 8 -s 100 -u admin");
    println!();
    println!("  # List all VMs");
    println!("  qvm ls");
    println!();
    println!("  # Get IP and SSH in");
    println!("  qvm ip web01");
    println!("  qvm ssh-cmd web01");
    println!();
    println!("  # Stop / start / delete");
    println!("  qvm stop web01");
    println!("  qvm start web01");
    println!("  qvm rm web01");
    println!();
    println!("  # Grow a disk");
    println!("  qvm resize-disk web01 +50G");
    println!();
    println!("  # VNC access (tunnel via SSH)");
    println!("  qvm vnc web01");
    println!("  # then on your laptop:");
    println!("  ssh -L 5901:127.0.0.1:5901 root@your-host");
}
