use crate::commands::pull::pull_one;
use crate::config::{self, Config};
use crate::error::{Error, Result};
use crate::util::{prompt, prompt_bool, prompt_u32};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub fn run(config_path: &Path, pull_all: bool, yes: bool) -> Result<()> {
    let wizard_wants_images = if !config_path.exists() {
        if yes {
            write_defaults(config_path)?;
            false
        } else {
            run_wizard(config_path)?
        }
    } else {
        println!("Config already exists at {} (leaving it alone).", config_path.display());
        println!("Edit it directly, or delete it and re-run `qvm init` to re-run setup.");
        false
    };

    let cfg = Config::load(Some(config_path))?;
    cfg.ensure_dirs()?;
    println!();
    println!("Directories:");
    println!("  images    {}", cfg.paths.images.display());
    println!("  vms       {}", cfg.paths.vms.display());
    println!("  cloudinit {}", cfg.paths.cloudinit.display());

    if pull_all || wizard_wants_images {
        pull_all_images(&cfg)?;
    } else {
        println!();
        println!("Next:");
        println!("  qvm pull debian:13        # download a single distro base image");
        println!("  qvm init --pull-all       # download all five built-in distros");
        println!("  qvm run myvm              # create your first VM");
    }
    Ok(())
}

/// Download every base image in `cfg.distros` that isn't already present.
/// Returns an error if ANY download fails (lists which distros failed).
fn pull_all_images(cfg: &Config) -> Result<()> {
    crate::cmd::require("wget")?;
    println!();
    println!("Downloading baseline images ({} distros)...", cfg.distros.len());

    let mut failed: Vec<(String, String)> = Vec::new();
    for (key, d) in &cfg.distros {
        let dest = cfg.paths.images.join(&d.image);
        if dest.exists() {
            println!("  [skip] {key:16}  already present");
            continue;
        }
        println!("  [pull] {key:16}  {}", d.url);
        match pull_one(cfg, key) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("  [fail] {key}: {e}");
                failed.push((key.clone(), e.to_string()));
            }
        }
    }

    if failed.is_empty() {
        println!();
        println!("All baseline images downloaded.");
        return Ok(());
    }
    Err(Error::User(format!(
        "{}/{} image download(s) failed: {}",
        failed.len(), cfg.distros.len(),
        failed.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(", "),
    )))
}

// ── wizard ────────────────────────────────────────────────────────────────────

/// Returns true when the user opted to download the base images now.
fn run_wizard(config_path: &Path) -> Result<bool> {
    println!();
    println!("╔══════════════════════════════════════════════════╗");
    println!("║          qvm  —  first-time setup                ║");
    println!("╚══════════════════════════════════════════════════╝");
    println!();
    println!("No config found at {}", config_path.display());
    println!("Press Enter to accept the default shown in [brackets].");
    println!();

    println!("── Network ─────────────────────────────────────────");
    let bridge = prompt("Bridge interface (must exist on this host)", "br0");

    println!();
    println!("── VM defaults ──────────────────────────────────────");
    println!("  Built-in distros: ubuntu:24.04  debian:13  fedora:42  alpine:3.20  rocky:9");
    let distro    = prompt("Default distro", "debian:13");
    let cpus      = prompt_u32("Default CPUs", 2);
    let memory_gb = prompt_u32("Default RAM (GB)", 4);
    let disk_gb   = prompt_u32("Default disk (GB)", 50);
    let autostart = prompt_bool("Autostart VMs on host boot?", true);

    println!();
    println!("── Boot ─────────────────────────────────────────────");
    let grub_timeout = prompt_u32("GRUB timeout in seconds (0 = instant boot)", 0);

    println!();
    println!("── VNC ──────────────────────────────────────────────");
    println!("  127.0.0.1 = localhost only (tunnel via SSH)  |  0.0.0.0 = expose on LAN");
    let vnc_bind = prompt("VNC bind address", "127.0.0.1");

    println!();
    println!("── SSH keys ─────────────────────────────────────────");
    println!("  Keys are injected into every VM for both the login user and root.");
    println!("  Paste one key per line. Empty line when done.");
    let ssh_keys = collect_ssh_keys();

    println!();
    println!("── Storage paths ────────────────────────────────────");
    let images_path    = prompt("Base image cache dir", "/var/lib/qvm/images");
    let vms_path       = prompt("VM disk dir",          "/var/lib/qvm/vms");
    let cloudinit_path = prompt("Cloud-init seed dir",  "/var/lib/qvm/cloudinit");

    let toml = render_config(WizardAnswers {
        bridge: &bridge,
        distro: &distro,
        cpus,
        memory_gb,
        disk_gb,
        autostart,
        grub_timeout,
        vnc_bind: &vnc_bind,
        ssh_keys: &ssh_keys,
        images_path: &images_path,
        vms_path: &vms_path,
        cloudinit_path: &cloudinit_path,
    });

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(config_path, &toml)?;

    println!();
    println!("Config written to {}", config_path.display());

    println!();
    println!("── Base images ──────────────────────────────────────");
    println!("  The 5 built-in distro images total ~2 GB. You can also pull");
    println!("  them later with `qvm pull <distro>` or `qvm init --pull-all`.");
    let want_images = prompt_bool("Download all 5 base images now?", false);
    Ok(want_images)
}

struct WizardAnswers<'a> {
    bridge:         &'a str,
    distro:         &'a str,
    cpus:           u32,
    memory_gb:      u32,
    disk_gb:        u32,
    autostart:      bool,
    grub_timeout:   u32,
    vnc_bind:       &'a str,
    ssh_keys:       &'a [String],
    images_path:    &'a str,
    vms_path:       &'a str,
    cloudinit_path: &'a str,
}

fn render_config(a: WizardAnswers<'_>) -> String {
    let keys_toml = if a.ssh_keys.is_empty() {
        "    # \"ssh-ed25519 AAAA... you@host\",\n".to_string()
    } else {
        a.ssh_keys.iter()
            .map(|k| format!("    \"{}\",\n", toml_escape(k)))
            .collect()
    };

    let autostart_str = if a.autostart { "true" } else { "false" };

    format!(
"# qvm configuration — generated by `qvm init`.
# Edit freely; all fields are optional (baked-in defaults apply if omitted).

[paths]
images    = \"{images}\"
vms       = \"{vms}\"
cloudinit = \"{ci}\"

[network]
bridge = \"{bridge}\"

[defaults]
distro       = \"{distro}\"
cpus         = {cpus}
memory_gb    = {mem}
disk_gb      = {disk}
autostart    = {autostart}
# 0 = instant boot. Comment out the line to keep the distro default.
grub_timeout = {grub}

# Username and password are required per-VM at create time; qvm has no
# default for either. Pass them on the CLI (`-u`/`-p`) or in the TUI
# Create form. SSH keys (below) are still global.

[vnc]
bind = \"{vnc}\"

# SSH public keys injected for the login user and root on every new VM.
ssh_keys = [
{keys}]

# ── Custom distros (optional) ────────────────────────────────────────────────
# Add or override built-in distros. Built-ins:
#   ubuntu:24.04   debian:13   fedora:42   alpine:3.20   rocky:9
#
# [distros.\"ubuntu:22.04\"]
# image  = \"ubuntu-22.04.qcow2\"
# osinfo = \"ubuntu22.04\"
# shell  = \"/bin/bash\"
# uefi   = false
# url    = \"https://cloud-images.ubuntu.com/releases/jammy/release/ubuntu-22.04-server-cloudimg-amd64.img\"
",
        images  = toml_escape(a.images_path),
        vms     = toml_escape(a.vms_path),
        ci      = toml_escape(a.cloudinit_path),
        bridge  = toml_escape(a.bridge),
        distro  = toml_escape(a.distro),
        cpus    = a.cpus,
        mem     = a.memory_gb,
        disk    = a.disk_gb,
        autostart = autostart_str,
        grub    = a.grub_timeout,
        vnc     = toml_escape(a.vnc_bind),
        keys    = keys_toml,
    )
}

/// Escape a string for use inside a TOML basic (double-quoted) string.
fn toml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"'  => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn write_defaults(config_path: &Path) -> Result<()> {
    if let Some(parent) = config_path.parent() { fs::create_dir_all(parent)?; }
    fs::write(config_path, config::sample_toml())?;
    println!("Wrote default config to {}", config_path.display());
    Ok(())
}

fn collect_ssh_keys() -> Vec<String> {
    let mut keys = Vec::new();
    let mut n = 1u32;
    loop {
        print!("  Key {n} (empty to finish): ");
        io::stdout().flush().ok();
        let mut line = String::new();
        io::stdin().read_line(&mut line).ok();
        let t = line.trim().to_string();
        if t.is_empty() { break; }
        keys.push(t);
        n += 1;
    }
    keys
}
