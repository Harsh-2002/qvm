use crate::error::{Error, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_CONFIG_PATH: &str = "/etc/qvm/config.toml";

/// Top-level config. Every field has a default, so the entire file is optional.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub paths: Paths,
    pub network: Network,
    pub defaults: Defaults,
    pub vnc: Vnc,
    pub ssh_keys: Vec<String>,
    /// Distro registry. Key = "name:version" (docker-style).
    /// Empty = use the baked-in defaults.
    pub distros: BTreeMap<String, Distro>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Paths {
    pub images: PathBuf,
    pub vms: PathBuf,
    pub cloudinit: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Network {
    pub bridge: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub distro: String,
    pub cpus: u32,
    pub memory_gb: u32,
    pub disk_gb: u32,
    pub autostart: bool,
    pub grub_timeout: Option<u32>,
    /// Default for nested virtualization in new VMs. `true` means guests
    /// see vmx/svm via `--cpu host-passthrough` and can themselves run
    /// KVM. `false` switches to `host-model,-vmx,-svm`. Overridable per
    /// VM via `qvm run --no-nested`.
    pub nested: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Vnc {
    /// "127.0.0.1" (default) or "0.0.0.0" to expose on LAN.
    pub bind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Distro {
    /// Filename under [paths].images
    pub image: String,
    /// libosinfo id (passed with require=off, so never fatal).
    pub osinfo: String,
    /// Login shell. Default /bin/bash; Alpine uses /bin/sh.
    #[serde(default = "default_shell")]
    pub shell: String,
    /// UEFI required for boot? (Alpine cloud image yes, others no.)
    #[serde(default)]
    pub uefi: bool,
    /// Download URL for `qvm pull`. Should be a STABLE release, not a daily.
    pub url: String,
}

fn default_shell() -> String { "/bin/bash".into() }

// --- defaults baked into the binary -----------------------------------------

impl Default for Paths {
    fn default() -> Self {
        Self {
            images:    "/var/lib/qvm/images".into(),
            vms:       "/var/lib/qvm/vms".into(),
            cloudinit: "/var/lib/qvm/cloudinit".into(),
        }
    }
}

impl Default for Network {
    fn default() -> Self { Self { bridge: "br0".into() } }
}

impl Default for Defaults {
    fn default() -> Self {
        // Numeric defaults are fine. Username + password are intentionally
        // NOT defaulted anywhere — every `qvm run` requires `-u` and `-p`.
        Self {
            distro: "debian:13".into(),
            cpus: 2,
            memory_gb: 4,
            disk_gb: 50,
            autostart: true,
            grub_timeout: Some(0),
            nested: true,
        }
    }
}

impl Default for Vnc {
    fn default() -> Self { Self { bind: "127.0.0.1".into() } }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            paths: Paths::default(),
            network: Network::default(),
            defaults: Defaults::default(),
            vnc: Vnc::default(),
            ssh_keys: vec![],
            distros: builtin_distros(),
        }
    }
}

/// Five well-known distros, all on STABLE release URLs (not dailies).
/// These cannot be silently re-rolled by upstream into a broken VM
/// because every qvm-created VM disk is a full copy, not an overlay.
pub fn builtin_distros() -> BTreeMap<String, Distro> {
    let mut m = BTreeMap::new();

    m.insert("ubuntu:24.04".into(), Distro {
        image:  "ubuntu-24.04.qcow2".into(),
        osinfo: "ubuntu24.04".into(),
        shell:  "/bin/bash".into(),
        uefi:   false,
        url:    "https://cloud-images.ubuntu.com/releases/noble/release/ubuntu-24.04-server-cloudimg-amd64.img".into(),
    });

    m.insert("debian:13".into(), Distro {
        image:  "debian-13.qcow2".into(),
        osinfo: "debian12".into(),
        shell:  "/bin/bash".into(),
        uefi:   false,
        url:    "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2".into(),
    });

    m.insert("fedora:42".into(), Distro {
        image:  "fedora-42.qcow2".into(),
        osinfo: "fedora41".into(),
        shell:  "/bin/bash".into(),
        uefi:   false,
        url:    "https://download.fedoraproject.org/pub/fedora/linux/releases/42/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-42-1.1.x86_64.qcow2".into(),
    });

    m.insert("alpine:3.20".into(), Distro {
        image:  "alpine-3.20.qcow2".into(),
        osinfo: "alpinelinux3.20".into(),
        shell:  "/bin/sh".into(),
        uefi:   true,
        url:    "https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/cloud/nocloud_alpine-3.20.3-x86_64-uefi-cloudinit-r0.qcow2".into(),
    });

    m.insert("rocky:9".into(), Distro {
        image:  "rocky-9.qcow2".into(),
        osinfo: "rocky9".into(),
        shell:  "/bin/bash".into(),
        uefi:   false,
        url:    "https://download.rockylinux.org/pub/rocky/9/images/x86_64/Rocky-9-GenericCloud-Base.latest.x86_64.qcow2".into(),
    });

    m
}

// --- loader -----------------------------------------------------------------

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let p = path.unwrap_or_else(|| Path::new(DEFAULT_CONFIG_PATH));
        let mut cfg = if p.exists() {
            let raw = fs::read_to_string(p)?;
            let parsed: Config = toml::from_str(&raw)?;
            parsed
        } else {
            Config::default()
        };
        // If user defined no distros in the file, give them the baked-in set.
        if cfg.distros.is_empty() {
            cfg.distros = builtin_distros();
        }
        Ok(cfg)
    }

    /// Resolve `<images>/<filename>` for a given distro key.
    pub fn image_path(&self, distro: &str) -> Result<PathBuf> {
        let d = self.distros.get(distro).ok_or_else(|| Error::User(
            format!("unknown distro '{distro}'. Run `qvm distros` to list available.")
        ))?;
        Ok(self.paths.images.join(&d.image))
    }

    /// Per-VM disk file `<vms>/<name>.qcow2`.
    pub fn vm_disk(&self, name: &str) -> PathBuf {
        self.paths.vms.join(format!("{name}.qcow2"))
    }

    /// Per-VM cloud-init seed ISO `<cloudinit>/<name>.iso`.
    pub fn vm_seed_iso(&self, name: &str) -> PathBuf {
        self.paths.cloudinit.join(format!("{name}.iso"))
    }

    /// Per-VM cloud-init working directory `<cloudinit>/<name>/`.
    pub fn vm_ci_dir(&self, name: &str) -> PathBuf {
        self.paths.cloudinit.join(name)
    }

    pub fn distro(&self, key: &str) -> Result<&Distro> {
        self.distros.get(key).ok_or_else(|| Error::User(
            format!("unknown distro '{key}'. Run `qvm distros` to list available.")
        ))
    }

    /// Ensure /var/lib/qvm/{images,vms,cloudinit} exist (root-owned, 0700).
    pub fn ensure_dirs(&self) -> Result<()> {
        for p in [&self.paths.images, &self.paths.vms, &self.paths.cloudinit] {
            fs::create_dir_all(p)?;
        }
        Ok(())
    }
}

/// Render the bootstrap config that `qvm init` writes if none exists.
pub fn sample_toml() -> &'static str {
    include_str!("config.sample.toml")
}
