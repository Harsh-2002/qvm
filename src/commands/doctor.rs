//! `qvm doctor` - check host dependencies and optionally install them.

use crate::cmd::{have, run, run_inherit};
use crate::error::{Error, Result};
use crate::util::{self, prompt_bool};
use std::fs;

/// Every external program qvm depends on, plus what package provides it
/// on each distro family.
#[derive(Debug, Clone, Copy)]
pub struct Dep {
    pub binary: &'static str,
    pub why: &'static str,
    pub packages: Packages,
}

#[derive(Debug, Clone, Copy)]
pub struct Packages {
    pub apt:    &'static str,  // debian, ubuntu
    pub dnf:    &'static str,  // fedora, rocky, alma, rhel
    pub apk:    &'static str,  // alpine
    pub pacman: &'static str,  // arch
}

/// Arch-independent dependencies. The arch-dependent qemu binary is
/// appended at runtime in [`deps_for_host`].
pub const DEPS_BASE: &[Dep] = &[
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
        binary: "genisoimage",
        why: "build the cloud-init seed ISO (NoCloud datasource)",
        packages: Packages {
            apt:    "genisoimage",
            dnf:    "genisoimage",
            apk:    "cdrkit",
            pacman: "cdrkit",
        },
    },
];

/// Per-arch QEMU emulator dep, picked from [`crate::arch::host()`].
const QEMU_X86_64: Dep = Dep {
    binary: "qemu-system-x86_64",
    why: "the actual VM emulator (KVM acceleration on amd64)",
    packages: Packages {
        apt:    "qemu-system-x86",
        dnf:    "qemu-kvm",
        apk:    "qemu-system-x86_64",
        pacman: "qemu-system-x86",
    },
};

const QEMU_AARCH64: Dep = Dep {
    binary: "qemu-system-aarch64",
    why: "the actual VM emulator (KVM acceleration on arm64)",
    packages: Packages {
        apt:    "qemu-system-arm",
        dnf:    "qemu-system-aarch64",
        apk:    "qemu-system-aarch64",
        pacman: "qemu-system-aarch64",
    },
};

/// All deps the running host needs, including the right qemu emulator.
pub fn deps_for_host() -> Vec<Dep> {
    let qemu = if crate::arch::is_arm() { QEMU_AARCH64 } else { QEMU_X86_64 };
    let mut out: Vec<Dep> = DEPS_BASE.to_vec();
    out.push(qemu);
    out
}

/// Back-compat alias: tests + `tui::onboard` still reference `DEPS`. We
/// can't constify `deps_for_host()`, so keep DEPS as the static slice and
/// expose `deps_for_host` for actual runtime listing.
pub const DEPS: &[Dep] = DEPS_BASE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Debian,    // debian, ubuntu, mint, ...
    Fedora,    // fedora, rocky, alma, rhel, centos
    Alpine,
    Arch,
    Unknown,
}

impl Family {
    pub fn from_os_release() -> Self {
        let raw = match fs::read_to_string("/etc/os-release") {
            Ok(s) => s,
            Err(_) => return Family::Unknown,
        };
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

    pub fn install_cmd(&self) -> Option<&'static str> {
        match self {
            Family::Debian => Some("apt-get install -y"),
            Family::Fedora => Some("dnf install -y"),
            Family::Alpine => Some("apk add"),
            Family::Arch   => Some("pacman -S --noconfirm"),
            Family::Unknown => None,
        }
    }

    pub fn packages<'a>(&self, p: &'a Packages) -> &'a str {
        match self {
            Family::Debian => p.apt,
            Family::Fedora => p.dnf,
            Family::Alpine => p.apk,
            Family::Arch   => p.pacman,
            Family::Unknown=> p.apt,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Family::Debian => "Debian/Ubuntu",
            Family::Fedora => "Fedora/RHEL/Rocky/Alma",
            Family::Alpine => "Alpine",
            Family::Arch   => "Arch",
            Family::Unknown=> "unknown distro",
        }
    }
}

fn libvirtd_ok() -> bool {
    run("virsh", ["-c", "qemu:///system", "list"]).is_ok()
}

pub fn run_doctor(install: bool, assume_yes: bool) -> Result<()> {
    use crate::style as s;
    let family = Family::from_os_release();
    println!("{} {}  ({} host)", s::label("Host:"), family.name(), crate::arch::host());
    println!();

    // 1. Binary presence check (arch-aware: picks the right qemu-system-*)
    let deps = deps_for_host();
    println!("{}", s::label("Dependencies:"));
    let mut missing: Vec<Dep> = Vec::new();
    let mut virsh_present = false;
    for d in &deps {
        let present = have(d.binary);
        if d.binary == "virsh" { virsh_present = present; }
        if present {
            println!("  {} {:<22} {}", s::ok("✓"), d.binary, s::dim(d.why));
        } else {
            println!("  {} {:<22} {}", s::err("✗"), d.binary, s::dim(d.why));
            missing.push(*d);
        }
    }

    // 2. libvirtd reachability (only meaningful if virsh is present)
    println!();
    println!("{}", s::label("Services:"));
    if virsh_present {
        if libvirtd_ok() {
            println!("  {} libvirtd reachable", s::ok("✓"));
        } else {
            println!("  {} libvirtd not reachable. Enable with:", s::warn("!"));
            println!("    {}", s::cmd("systemctl enable --now libvirtd"));
        }
    } else {
        println!("  {} libvirtd check (virsh missing)", s::dim("○"));
    }

    // 3. Root check
    println!();
    if util::is_root() {
        println!("  {} running as root", s::ok("✓"));
    } else {
        println!("  {} qvm must be run as root for VM operations", s::warn("!"));
    }

    if missing.is_empty() {
        println!();
        println!("{}", s::ok("All required dependencies are installed."));
        print_examples();
        return Ok(());
    }

    // 4. Install hint or actual install
    println!();
    println!("{} {} package(s).", s::warn("Missing"), missing.len());

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
        use crate::style as s;
        println!();
        println!("{}", s::label("Run this to install everything:"));
        println!("  {}", s::cmd(&full));
        println!();
        println!("Or let qvm do it: {}", s::cmd("qvm doctor --install"));
        return Err(Error::User("dependencies missing".into()));
    }

    if !util::is_root() {
        return Err(Error::User("--install requires root".into()));
    }
    println!();
    println!("About to run:");
    println!("  {full}");
    if !assume_yes && !prompt_bool("Proceed?", false) {
        return Err(Error::User("aborted".into()));
    }

    // apt is the common case that needs an explicit metadata refresh.
    if family == Family::Debian {
        let _ = run_inherit("apt-get", ["update"]);
    }

    let mut parts = install_cmd.split_whitespace();
    let prog = parts.next().unwrap();
    let cmd_args: Vec<&str> = parts.collect();
    let mut final_args: Vec<&str> = cmd_args;
    for p in &pkgs { final_args.push(p); }

    run_inherit(prog, final_args)?;

    println!();
    println!("Re-running checks...");
    run_doctor(false, assume_yes)
}

fn print_examples() {
    use crate::style as s;
    println!();
    println!("{}", s::label("Next:"));
    println!("  {}    {}", s::cmd("qvm"),                 s::dim("# opens the TUI"));
    println!("  {} <vm> <distro> -u <user> -p <pw>   {}", s::cmd("qvm run"), s::dim("# create a VM"));
    println!("  {} <vm> --browser                    {}", s::cmd("qvm vnc"), s::dim("# console in a browser"));
    println!("  {}                                  {}",  s::cmd("qvm --help"), s::dim("# every command"));
}
