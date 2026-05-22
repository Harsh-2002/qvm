//! cloud-init seed generation.
//!
//! We emit a single user-data that:
//!   * creates the login user + root with SSH keys and password
//!   * installs and enables qemu-guest-agent (generic across init systems)
//!   * configures GRUB timeout (default 0 = instant boot)
//!
//! All cross-distro behaviour is in a small first-boot script that
//! feature-detects systemctl vs rc-update etc. - NO per-distro branching here.

use crate::cmd::{require, run};
use crate::config::{Config, Motd};
use crate::error::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Seed<'a> {
    pub vm_name:      &'a str,
    pub login_user:   &'a str,
    pub login_shell:  &'a str,
    pub password_hash:&'a str,
    pub ssh_keys:     &'a [String],
    pub grub_timeout: Option<u32>,
    /// When `Some`, qvm installs `/etc/profile.d/qvm-motd.sh` and the
    /// firstboot script silences the distro default banners. `None`
    /// keeps the distro's stock MOTD untouched.
    pub motd:         Option<&'a Motd>,
    /// Run `package_update` + `package_upgrade` on first boot. Adds
    /// 1–5 min to first boot; off by default.
    pub upgrade:      bool,
    /// When set, the firstboot script creates a persistent /swapfile
    /// of this size in megabytes and adds it to /etc/fstab.
    pub swap_mb:      Option<u64>,
    /// When `Some`, qvm writes a NoCloud `network-config` v2 file
    /// alongside user-data/meta-data. Cloud-init translates it into
    /// the distro-native networking file on first boot. `None` →
    /// DHCP (today's behaviour).
    pub network:      Option<NetworkCfg<'a>>,
}

/// Static-IP configuration baked into the seed. We only emit a
/// `network-config` file when this is supplied; absence means
/// "cloud-init falls through to its DHCP default", which is the
/// existing behaviour for every VM created before this feature.
pub struct NetworkCfg<'a> {
    /// IPv4 in CIDR form, e.g. `"10.1.1.50/24"`.
    pub ip_cidr: &'a str,
    /// IPv4 default gateway, e.g. `"10.1.1.1"`.
    pub gateway: &'a str,
    /// DNS resolvers. Caller is responsible for falling back to a
    /// public default if the operator's config has none set —
    /// `cloudinit::motd` must never emit an empty nameservers list,
    /// which would leave the VM without DNS at first boot.
    pub dns:     &'a [String],
}

impl<'a> Seed<'a> {
    /// Write user-data + meta-data + .vmuser sidecar into `ci_dir`.
    /// Does not require genisoimage; usable in tests and as a recovery point
    /// if the ISO step later fails.
    pub fn write_files(&self, ci_dir: &Path) -> Result<()> {
        fs::create_dir_all(ci_dir)?;
        fs::write(ci_dir.join(".vmuser"), format!("{}\n", self.login_user))?;
        fs::write(ci_dir.join("user-data"), self.user_data())?;
        fs::write(ci_dir.join("meta-data"), format!(
            "instance-id: {0}\nlocal-hostname: {0}\n", self.vm_name
        ))?;
        if let Some(net) = &self.network {
            fs::write(ci_dir.join("network-config"), render_network_config(net))?;
        }
        Ok(())
    }

    /// Build the seed ISO from files already in `ci_dir`. Requires genisoimage.
    pub fn build_iso(&self, ci_dir: &Path, iso_path: &Path) -> Result<()> {
        require("genisoimage")?;
        let mut args: Vec<String> = vec![
            "-quiet".into(), "-output".into(), iso_path.to_str().unwrap().into(),
            "-volid".into(), "cidata".into(), "-joliet".into(), "-rock".into(),
            ci_dir.join("user-data").to_str().unwrap().into(),
            ci_dir.join("meta-data").to_str().unwrap().into(),
        ];
        // When static networking is set, include the v2 network-config
        // as the third payload file in the cidata volume.
        if self.network.is_some() {
            args.push(ci_dir.join("network-config").to_str().unwrap().into());
        }
        run("genisoimage", args.iter().map(|s| s.as_str()))?;
        Ok(())
    }

    /// Convenience: write files + build the ISO in one step. Returns the
    /// ISO path. This is what create_vm calls in normal operation.
    pub fn build(&self, ci_dir: &Path, iso_path: &Path) -> Result<PathBuf> {
        self.write_files(ci_dir)?;
        self.build_iso(ci_dir, iso_path)?;
        Ok(iso_path.to_path_buf())
    }

    fn user_data(&self) -> String {
        let mut keys_yaml = String::new();
        for k in self.ssh_keys {
            keys_yaml.push_str(&format!("      - {k}\n"));
        }

        let install_motd = self.motd.is_some();
        let firstboot = firstboot_script(self.grub_timeout, install_motd, self.swap_mb);
        let firstboot_indented = indent_six(&firstboot);

        // Optional second `write_files:` entry for the MOTD script.
        let motd_entry = if let Some(m) = self.motd {
            let body = motd_script(m);
            let body_indented = indent_six(&body);
            format!(
"  - path: /etc/profile.d/qvm-motd.sh
    permissions: '0755'
    content: |
{body}\n",
                body = body_indented,
            )
        } else {
            String::new()
        };

        // Opt-in `package_update` + `package_upgrade` at the top level
        // of cloud-config. Cloud-init runs them in the right order
        // (update → upgrade → packages) on every supported package
        // manager.
        let upgrade_block = if self.upgrade {
            "package_update:  true\npackage_upgrade: true\n"
        } else {
            ""
        };

        format!(
"#cloud-config
hostname: {name}
manage_etc_hosts: true
ssh_pwauth: true
packages: [qemu-guest-agent]
{upgrade_block}ntp:
  enabled: true
  servers:
    - 0.pool.ntp.org
    - 1.pool.ntp.org
    - 2.pool.ntp.org
    - 3.pool.ntp.org
users:
  - name: {user}
    groups: sudo
    shell: {shell}
    sudo: ALL=(ALL) NOPASSWD:ALL
    lock_passwd: false
    passwd: \"{pw}\"
    ssh-authorized-keys:
{keys}  - name: root
    ssh-authorized-keys:
{keys}write_files:
  - path: /opt/qvm-firstboot.sh
    permissions: '0755'
    content: |
{firstboot}
{motd_entry}runcmd:
  - /opt/qvm-firstboot.sh
",
            name = self.vm_name,
            user = self.login_user,
            shell = self.login_shell,
            pw = self.password_hash,
            keys = if keys_yaml.is_empty() { "      []\n".to_string() } else { keys_yaml },
            firstboot = firstboot_indented,
            motd_entry = motd_entry,
            upgrade_block = upgrade_block,
        )
    }
}

/// Render the NoCloud `network-config` v2 file. Uses modern netplan
/// syntax (`routes:` form, not the deprecated `gateway4:`). Cloud-
/// init translates this into the distro-native file on first boot.
fn render_network_config(n: &NetworkCfg) -> String {
    let dns_list: String = n.dns.iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
"# Generated by qvm — cloud-init translates this into the distro-native
# network config (netplan on Ubuntu, /etc/network/interfaces on Debian/
# Alpine, NetworkManager keyfile on Fedora/Rocky/Alma/openSUSE, etc.).
version: 2
ethernets:
  primary:
    match: {{ name: \"e*\" }}
    dhcp4: false
    addresses: [{ip}]
    routes:
      - to: default
        via: {gw}
    nameservers:
      addresses: [{dns}]
",
        ip = n.ip_cidr,
        gw = n.gateway,
        dns = dns_list,
    )
}

/// Indent every line by 6 spaces — matches the YAML block-scalar
/// indentation `user_data()` uses for embedded shell scripts.
fn indent_six(s: &str) -> String {
    s.lines().map(|l| format!("      {l}")).collect::<Vec<_>>().join("\n")
}

/// Generic first-boot script. Feature-detects rather than per-distro branches.
fn firstboot_script(
    grub_timeout: Option<u32>,
    silence_default_motd: bool,
    swap_mb: Option<u64>,
) -> String {
    let t = grub_timeout.map(|n| n.to_string()).unwrap_or_default();
    let motd_block = if silence_default_motd {
        r#"
# --- silence default MOTD spam so /etc/profile.d/qvm-motd.sh stands alone ---
if [ -d /etc/update-motd.d ]; then
    chmod -x /etc/update-motd.d/* 2>/dev/null || true
fi
[ -f /etc/motd ]         && : > /etc/motd 2>/dev/null || true
[ -f /etc/motd.dynamic ] && : > /etc/motd.dynamic 2>/dev/null || true
"#
    } else {
        ""
    };
    let swap_block = match swap_mb {
        Some(n) if n > 0 => format!(r#"
# --- persistent swap (created by qvm at first boot) ---
SWAP_MB="{n}"
if [ ! -e /swapfile ]; then
    dd if=/dev/zero of=/swapfile bs=1M count="$SWAP_MB" status=none 2>/dev/null || true
    chmod 600 /swapfile 2>/dev/null || true
    if command -v mkswap >/dev/null 2>&1; then
        mkswap /swapfile >/dev/null 2>&1 || true
        swapon /swapfile 2>/dev/null || true
        grep -q '^/swapfile ' /etc/fstab \
            || echo '/swapfile none swap sw 0 0' >> /etc/fstab
    fi
fi
"#),
        _ => String::new(),
    };
    format!(r#"#!/bin/sh
# Generated by qvm - runs once on first boot.

# --- GRUB timeout (best-effort across distros) ---
T="{t}"
if [ -n "$T" ]; then
    if [ -f /etc/default/grub ] && grep -q '^GRUB_TIMEOUT=' /etc/default/grub; then
        sed -i "s/^GRUB_TIMEOUT=.*/GRUB_TIMEOUT=$T/" /etc/default/grub
    else
        echo "GRUB_TIMEOUT=$T" >> /etc/default/grub
    fi
    if command -v update-grub >/dev/null 2>&1; then update-grub || true
    elif command -v grub2-mkconfig >/dev/null 2>&1; then
        [ -d /boot/grub2 ] && grub2-mkconfig -o /boot/grub2/grub.cfg || grub2-mkconfig -o /boot/grub/grub.cfg || true
    elif command -v grub-mkconfig >/dev/null 2>&1; then
        grub-mkconfig -o /boot/grub/grub.cfg || true
    fi
fi

# --- qemu-guest-agent: systemd OR openrc ---
if command -v systemctl >/dev/null 2>&1; then
    systemctl enable --now qemu-guest-agent || true
elif command -v rc-update >/dev/null 2>&1; then
    rc-update add qemu-guest-agent default || true
    rc-service qemu-guest-agent start || true
fi
{swap_block}{motd_block}"#)
}

/// Embedded template for `/etc/profile.d/qvm-motd.sh`. Lives in its own
/// file so it can be syntax-checked + container-tested standalone.
const MOTD_SCRIPT_TEMPLATE: &str = include_str!("motd.sh");

/// Render the MOTD shell script with the user's chosen colour mode +
/// palette substituted into the canonical template lines. Plain string
/// replacement — no templating engine, no per-distro branching.
fn motd_script(m: &Motd) -> String {
    let mut s = MOTD_SCRIPT_TEMPLATE.to_string();
    // Each substitution targets the canonical line shipped in motd.sh.
    s = s.replace(
        "COLOR_MODE_DEFAULT=\"auto\"",
        &format!("COLOR_MODE_DEFAULT=\"{}\"", shell_single_quote_safe(&m.color)),
    );
    s = replace_esc_assign(&s, "LABEL_ESC", &m.colors.label);
    s = replace_esc_assign(&s, "BOLD_ESC",  &m.colors.bold);
    s = replace_esc_assign(&s, "OK_ESC",    &m.colors.ok);
    s = replace_esc_assign(&s, "WARN_ESC",  &m.colors.warn);
    s = replace_esc_assign(&s, "CRIT_ESC",  &m.colors.crit);
    s
}

/// Replace `<name>='[old]'` with `<name>='<new>'` in the canonical
/// motd.sh assignment line. The first single-quoted value on the line
/// is overwritten, leaving any trailing comment intact.
fn replace_esc_assign(s: &str, name: &str, value: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(&format!("{name}=")) {
            // Find the indent prefix + everything after the closing quote
            // (so the inline comment survives).
            let indent_end = line.len() - trimmed.len();
            let (indent, rest) = line.split_at(indent_end);
            let prefix = format!("{name}='");
            if let Some(after_open) = rest.strip_prefix(&prefix) {
                if let Some(close_idx) = after_open.find('\'') {
                    let tail = &after_open[close_idx + 1..];
                    out.push_str(indent);
                    out.push_str(&prefix);
                    out.push_str(&shell_single_quote_safe(value));
                    out.push('\'');
                    out.push_str(tail);
                    out.push('\n');
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Defang a user-supplied palette value so it can be embedded between
/// single quotes in shell. The escape sequences we expect (`[0;36m`)
/// don't contain `'`, but a hostile config shouldn't crash the cloud-
/// init pipeline — escape any literal single-quote with `'\''`.
fn shell_single_quote_safe(s: &str) -> String { s.replace('\'', "'\\''") }

/// Read back the persisted login user for a VM (None if missing/unreadable).
pub fn login_user_of(cfg: &Config, vm: &str) -> Option<String> {
    let p = cfg.paths.cloudinit.join(vm).join(".vmuser");
    fs::read_to_string(p).ok().and_then(|s| {
        let line = s.lines().next()?.trim().to_string();
        if line.is_empty() { None } else { Some(line) }
    })
}
