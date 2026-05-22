use crate::error::{Error, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_CONFIG_PATH: &str = "/etc/qvm/config.yml";

/// Top-level config. Every field has a default, so the entire file is optional.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub paths: Paths,
    pub network: Network,
    pub defaults: Defaults,
    pub vnc: Vnc,
    pub tui: Tui,
    pub motd: Motd,
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
    /// Upstream DNS resolvers used for new VMs with a static IP
    /// (`qvm run --ip ...`). Empty = qvm falls back to 1.1.1.1 + 8.8.8.8
    /// at create time so a static VM is never DNS-broken from boot.
    pub dns:    Vec<String>,
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
#[serde(default)]
pub struct Tui {
    /// Theme name. `"mocha"` (dark, default) or `"latte"` (light).
    pub theme: String,
}

/// MOTD installation knob.
///
/// When `enable = true` (the default), `qvm` drops a small POSIX shell
/// script into `/etc/profile.d/qvm-motd.sh` via the cloud-init seed,
/// and the first-boot script silences the distro's default banners
/// (`/etc/update-motd.d/*`, `/etc/motd`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Motd {
    pub enable: bool,
    /// `"auto"` (respects NO_COLOR + TTY), `"always"`, or `"never"`.
    pub color: String,
    /// Optional palette override. Values are ANSI escape sequences
    /// WITHOUT the leading `\033` (e.g. `"[0;36m"` for cyan). Any
    /// subset of fields may be set; unset fields fall back to the
    /// baked-in defaults (16-colour ANSI).
    pub colors: MotdColors,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MotdColors {
    /// Field labels (`IP:`, `Uptime:`, …).
    pub label: String,
    /// Bold attribute used for the hostname banner.
    pub bold:  String,
    /// CPU / RAM colour when below 60%.
    pub ok:    String,
    /// CPU / RAM colour when 60–79%.
    pub warn:  String,
    /// CPU / RAM colour when ≥ 80%.
    pub crit:  String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Distro {
    /// libosinfo id (passed with require=off, so never fatal).
    pub osinfo: String,
    /// Login shell. Default /bin/bash; Alpine uses /bin/sh.
    #[serde(default = "default_shell")]
    pub shell: String,
    /// UEFI required on x86? On aarch64 UEFI is mandatory regardless.
    #[serde(default)]
    pub uefi: bool,

    // ── Legacy single-arch form (kept for back-compat with old configs).
    //    These are read but no longer the canonical place to put the image
    //    name or URL. If both flat and per-arch fields are present, the
    //    per-arch ones win.
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub url:   Option<String>,

    /// Per-architecture variants. Keys are `uname -m` values:
    /// `"x86_64"`, `"aarch64"`. Built-ins populate both.
    #[serde(default)]
    pub arch:  BTreeMap<String, DistroVariant>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DistroVariant {
    pub image: String,
    pub url:   String,
}

impl Distro {
    /// Resolve `(image, url)` for the given host arch (`uname -m` value).
    /// Prefers the per-arch map; falls back to the legacy flat fields
    /// only when the host is `x86_64`.
    pub fn variant_for(&self, host_arch: &str) -> Result<(&str, &str)> {
        if let Some(v) = self.arch.get(host_arch) {
            return Ok((&v.image, &v.url));
        }
        if host_arch == "x86_64" {
            if let (Some(img), Some(url)) = (self.image.as_deref(), self.url.as_deref()) {
                return Ok((img, url));
            }
        }
        Err(Error::User(format!(
            "distro has no variant for arch '{host_arch}'. Add an `[distros.\"name\".arch.{host_arch}]` table."
        )))
    }
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
    fn default() -> Self { Self { bridge: "br0".into(), dns: vec![] } }
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

impl Default for Tui {
    fn default() -> Self { Self { theme: "mocha".into() } }
}

impl Default for Motd {
    fn default() -> Self {
        Self {
            enable: true,
            color:  "auto".into(),
            colors: MotdColors::default(),
        }
    }
}

impl Default for MotdColors {
    fn default() -> Self {
        // ANSI escapes WITHOUT the leading \033, matching the form the
        // shell script expects. 16-colour ANSI for portability.
        Self {
            label: "[0;36m".into(),  // cyan
            bold:  "[1m".into(),
            ok:    "[0;32m".into(),  // green
            warn:  "[1;33m".into(),  // yellow
            crit:  "[0;31m".into(),  // red
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            paths: Paths::default(),
            network: Network::default(),
            defaults: Defaults::default(),
            vnc: Vnc::default(),
            tui: Tui::default(),
            motd: Motd::default(),
            ssh_keys: vec![],
            distros: builtin_distros(),
        }
    }
}

/// Five well-known distros, all on STABLE release URLs (not dailies).
/// These cannot be silently re-rolled by upstream into a broken VM
/// because every qvm-created VM disk is a full copy, not an overlay.
///
/// Each distro carries both amd64 (`x86_64`) and arm64 (`aarch64`)
/// variants. `pull::pull_one` and `create::run` look the right one up
/// via `crate::arch::host()`.
pub fn builtin_distros() -> BTreeMap<String, Distro> {
    let mut m = BTreeMap::new();

    m.insert("ubuntu:24.04".into(), Distro {
        osinfo: "ubuntu24.04".into(),
        shell:  "/bin/bash".into(),
        uefi:   false,
        image:  None, url: None,
        arch:   variants(&[
            ("x86_64",  "ubuntu-24.04-amd64.qcow2",
             "https://cloud-images.ubuntu.com/releases/noble/release/ubuntu-24.04-server-cloudimg-amd64.img"),
            ("aarch64", "ubuntu-24.04-arm64.qcow2",
             "https://cloud-images.ubuntu.com/releases/noble/release/ubuntu-24.04-server-cloudimg-arm64.img"),
        ]),
    });

    m.insert("ubuntu:26.04".into(), Distro {
        osinfo: "ubuntu26.04".into(),
        shell:  "/bin/bash".into(),
        uefi:   false,
        image:  None, url: None,
        arch:   variants(&[
            ("x86_64",  "ubuntu-26.04-amd64.qcow2",
             "https://cloud-images.ubuntu.com/releases/26.04/release/ubuntu-26.04-server-cloudimg-amd64.img"),
            ("aarch64", "ubuntu-26.04-arm64.qcow2",
             "https://cloud-images.ubuntu.com/releases/26.04/release/ubuntu-26.04-server-cloudimg-arm64.img"),
        ]),
    });

    m.insert("debian:13".into(), Distro {
        osinfo: "debian12".into(),
        shell:  "/bin/bash".into(),
        uefi:   false,
        image:  None, url: None,
        arch:   variants(&[
            ("x86_64",  "debian-13-amd64.qcow2",
             "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2"),
            ("aarch64", "debian-13-arm64.qcow2",
             "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-arm64.qcow2"),
        ]),
    });

    m.insert("fedora:42".into(), Distro {
        osinfo: "fedora41".into(),
        shell:  "/bin/bash".into(),
        uefi:   false,
        image:  None, url: None,
        arch:   variants(&[
            ("x86_64",  "fedora-42-amd64.qcow2",
             "https://download.fedoraproject.org/pub/fedora/linux/releases/42/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-42-1.1.x86_64.qcow2"),
            ("aarch64", "fedora-42-arm64.qcow2",
             "https://download.fedoraproject.org/pub/fedora/linux/releases/42/Cloud/aarch64/images/Fedora-Cloud-Base-Generic-42-1.1.aarch64.qcow2"),
        ]),
    });

    m.insert("alpine:3.20".into(), Distro {
        osinfo: "alpinelinux3.20".into(),
        shell:  "/bin/sh".into(),
        uefi:   true,
        image:  None, url: None,
        arch:   variants(&[
            ("x86_64",  "alpine-3.20-amd64.qcow2",
             "https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/cloud/nocloud_alpine-3.20.3-x86_64-uefi-cloudinit-r0.qcow2"),
            ("aarch64", "alpine-3.20-arm64.qcow2",
             "https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/cloud/nocloud_alpine-3.20.3-aarch64-uefi-cloudinit-r0.qcow2"),
        ]),
    });

    m.insert("rocky:9".into(), Distro {
        osinfo: "rocky9".into(),
        shell:  "/bin/bash".into(),
        uefi:   false,
        image:  None, url: None,
        arch:   variants(&[
            ("x86_64",  "rocky-9-amd64.qcow2",
             "https://download.rockylinux.org/pub/rocky/9/images/x86_64/Rocky-9-GenericCloud-Base.latest.x86_64.qcow2"),
            ("aarch64", "rocky-9-arm64.qcow2",
             "https://download.rockylinux.org/pub/rocky/9/images/aarch64/Rocky-9-GenericCloud-Base.latest.aarch64.qcow2"),
        ]),
    });

    m.insert("almalinux:9".into(), Distro {
        osinfo: "almalinux9".into(),
        shell:  "/bin/bash".into(),
        uefi:   false,
        image:  None, url: None,
        arch:   variants(&[
            ("x86_64",  "almalinux-9-amd64.qcow2",
             "https://repo.almalinux.org/almalinux/9/cloud/x86_64/images/AlmaLinux-9-GenericCloud-latest.x86_64.qcow2"),
            ("aarch64", "almalinux-9-arm64.qcow2",
             "https://repo.almalinux.org/almalinux/9/cloud/aarch64/images/AlmaLinux-9-GenericCloud-latest.aarch64.qcow2"),
        ]),
    });

    m.insert("opensuse:15.6".into(), Distro {
        osinfo: "opensuseleap15.6".into(),
        shell:  "/bin/bash".into(),
        uefi:   false,
        image:  None, url: None,
        arch:   variants(&[
            ("x86_64",  "opensuse-15.6-amd64.qcow2",
             "https://download.opensuse.org/distribution/leap/15.6/appliances/openSUSE-Leap-15.6-Minimal-VM.x86_64-15.6.0-Cloud-Build19.146.qcow2"),
            ("aarch64", "opensuse-15.6-arm64.qcow2",
             "https://download.opensuse.org/distribution/leap/15.6/appliances/openSUSE-Leap-15.6-Minimal-VM.aarch64-15.6.0-Cloud-Build19.146.qcow2"),
        ]),
    });

    m.insert("centos-stream:10".into(), Distro {
        osinfo: "centos-stream10".into(),
        shell:  "/bin/bash".into(),
        uefi:   false,
        image:  None, url: None,
        arch:   variants(&[
            ("x86_64",  "centos-stream-10-amd64.qcow2",
             "https://cloud.centos.org/centos/10-stream/x86_64/images/CentOS-Stream-GenericCloud-10-latest.x86_64.qcow2"),
            ("aarch64", "centos-stream-10-arm64.qcow2",
             "https://cloud.centos.org/centos/10-stream/aarch64/images/CentOS-Stream-GenericCloud-10-latest.aarch64.qcow2"),
        ]),
    });

    // Arch ships only an x86_64 cloud image upstream and uses a rolling
    // "latest" URL — a deliberate exception to §3.5 "stable URLs not
    // dailies" because Arch has no pinned point releases for cloud images.
    m.insert("arch".into(), Distro {
        osinfo: "archlinux".into(),
        shell:  "/bin/bash".into(),
        uefi:   false,
        image:  None, url: None,
        arch:   variants(&[
            ("x86_64",  "arch-amd64.qcow2",
             "https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2"),
        ]),
    });

    m
}

fn variants(rows: &[(&str, &str, &str)]) -> BTreeMap<String, DistroVariant> {
    rows.iter().map(|(arch, img, url)| (
        (*arch).to_string(),
        DistroVariant { image: (*img).to_string(), url: (*url).to_string() },
    )).collect()
}

// --- loader -----------------------------------------------------------------

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let p = path.unwrap_or_else(|| Path::new(DEFAULT_CONFIG_PATH));
        let mut cfg = if p.exists() {
            let raw = fs::read_to_string(p)?;
            serde_yaml::from_str(&raw)?
        } else {
            Config::default()
        };
        // If user defined no distros in the file, give them the baked-in set.
        if cfg.distros.is_empty() {
            cfg.distros = builtin_distros();
        }
        Ok(cfg)
    }

    /// Resolve `<images>/<filename>` for a given distro key, picking the
    /// variant that matches this host's architecture.
    pub fn image_path(&self, distro: &str) -> Result<PathBuf> {
        let d = self.distros.get(distro).ok_or_else(|| Error::User(
            format!("unknown distro '{distro}'. Run `qvm distros` to list available.")
        ))?;
        let (image, _) = d.variant_for(crate::arch::host())?;
        Ok(self.paths.images.join(image))
    }

    /// Same idea for the download URL.
    pub fn image_url(&self, distro: &str) -> Result<String> {
        let d = self.distros.get(distro).ok_or_else(|| Error::User(
            format!("unknown distro '{distro}'. Run `qvm distros` to list available.")
        ))?;
        let (_, url) = d.variant_for(crate::arch::host())?;
        Ok(url.to_string())
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
pub fn sample_yaml() -> &'static str {
    include_str!("config.sample.yml")
}
