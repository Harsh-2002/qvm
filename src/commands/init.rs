use crate::cmd::run_inherit;
use crate::config::{self, Config};
use crate::error::Result;
use crate::util::hash_password;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;

pub fn run(config_path: &Path, pull_all: bool, yes: bool) -> Result<()> {
    if !config_path.exists() {
        if yes {
            write_defaults(config_path)?;
        } else {
            run_wizard(config_path)?;
        }
    } else {
        println!("Config already exists at {} (leaving it alone).", config_path.display());
        println!("Edit it directly, or delete it and re-run `qvm init` to re-run setup.");
    }

    let cfg = Config::load(Some(config_path))?;
    cfg.ensure_dirs()?;
    println!();
    println!("Directories:");
    println!("  images    {}", cfg.paths.images.display());
    println!("  vms       {}", cfg.paths.vms.display());
    println!("  cloudinit {}", cfg.paths.cloudinit.display());

    if pull_all {
        println!();
        println!("Downloading baseline images ({} distros)...", cfg.distros.len());
        for (key, d) in &cfg.distros {
            let dest = cfg.paths.images.join(&d.image);
            if dest.exists() {
                println!("  [skip] {key:16}  already present");
                continue;
            }
            println!("  [pull] {key:16}  {}", d.url);
            let tmp = dest.with_extension("partial");
            let _ = fs::remove_file(&tmp);
            let r = run_inherit("wget", [
                "-q", "--show-progress",
                d.url.as_str(),
                "-O", tmp.to_str().unwrap(),
            ]);
            match r {
                Ok(_) => { fs::rename(&tmp, &dest)?; }
                Err(e) => {
                    let _ = fs::remove_file(&tmp);
                    eprintln!("  [warn] {key}: {e}");
                }
            }
        }
    } else {
        println!();
        println!("Next:");
        println!("  qvm pull debian:13        # download a single distro base image");
        println!("  qvm init --pull-all       # download all five built-in distros");
        println!("  qvm run myvm              # create your first VM");
    }
    Ok(())
}

// ── wizard ────────────────────────────────────────────────────────────────────

fn run_wizard(config_path: &Path) -> Result<()> {
    let stdin = io::stdin();
    let mut input = stdin.lock();

    println!();
    println!("╔══════════════════════════════════════════════════╗");
    println!("║          qvm  —  first-time setup                ║");
    println!("╚══════════════════════════════════════════════════╝");
    println!();
    println!("No config found at {}", config_path.display());
    println!("Press Enter to accept the default shown in [brackets].");
    println!();

    // ── network ───────────────────────────────────────────────────────────────
    println!("── Network ─────────────────────────────────────────");
    let bridge = prompt(&mut input, "Bridge interface (must exist on this host)", "br0");

    // ── VM defaults ───────────────────────────────────────────────────────────
    println!();
    println!("── VM defaults ──────────────────────────────────────");
    println!("  Built-in distros: ubuntu:24.04  debian:13  fedora:42  alpine:3.20  rocky:9");
    let distro    = prompt(&mut input, "Default distro", "debian:13");
    let cpus      = prompt_u32(&mut input, "Default vCPUs", 2);
    let memory_gb = prompt_u32(&mut input, "Default RAM (GB)", 4);
    let disk_gb   = prompt_u32(&mut input, "Default disk (GB)", 50);
    let autostart = prompt_bool(&mut input, "Autostart VMs on host boot?", true);

    // ── GRUB ──────────────────────────────────────────────────────────────────
    println!();
    println!("── Boot ─────────────────────────────────────────────");
    let grub_timeout = prompt_u32(&mut input, "GRUB timeout in seconds (0 = instant boot)", 0);

    // ── VNC ───────────────────────────────────────────────────────────────────
    println!();
    println!("── VNC ──────────────────────────────────────────────");
    println!("  127.0.0.1 = localhost only (tunnel via SSH)  |  0.0.0.0 = expose on LAN");
    let vnc_bind = prompt(&mut input, "VNC bind address", "127.0.0.1");

    // ── SSH keys ──────────────────────────────────────────────────────────────
    println!();
    println!("── SSH keys ─────────────────────────────────────────");
    println!("  Keys are injected into every VM for both the login user and root.");
    println!("  Paste one key per line. Empty line when done.");
    let ssh_keys = collect_ssh_keys(&mut input);

    // ── default password ──────────────────────────────────────────────────────
    println!();
    println!("── Default VM password ──────────────────────────────");
    println!("  Used when `qvm run` is called without -p.");
    println!("  It is stored as a SHA-512 crypt hash in the config.");
    let pw_plain  = prompt(&mut input, "Default password", "changeme");
    let pw_hash   = hash_password(&pw_plain)?;

    // ── paths ─────────────────────────────────────────────────────────────────
    println!();
    println!("── Storage paths ────────────────────────────────────");
    let images_path    = prompt(&mut input, "Base image cache dir", "/var/lib/qvm/images");
    let vms_path       = prompt(&mut input, "VM disk dir",          "/var/lib/qvm/vms");
    let cloudinit_path = prompt(&mut input, "Cloud-init seed dir",  "/var/lib/qvm/cloudinit");

    // ── write config ──────────────────────────────────────────────────────────
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
        pw_hash: &pw_hash,
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
    println!("Edit it any time — run `qvm init --pull-all` to download images.");
    Ok(())
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
    pw_hash:        &'a str,
    images_path:    &'a str,
    vms_path:       &'a str,
    cloudinit_path: &'a str,
}

fn render_config(a: WizardAnswers<'_>) -> String {
    let keys_toml = if a.ssh_keys.is_empty() {
        "    # \"ssh-ed25519 AAAA... you@host\",\n".to_string()
    } else {
        a.ssh_keys.iter()
            .map(|k| format!("    \"{k}\",\n"))
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
grub_timeout = {grub}

# Default VM password hash (SHA-512 crypt). Override per-VM with `qvm run -p`.
# Regenerate: mkpasswd --method=SHA-512  OR  openssl passwd -6
password_hash = \"{pw_hash}\"

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
        images  = a.images_path,
        vms     = a.vms_path,
        ci      = a.cloudinit_path,
        bridge  = a.bridge,
        distro  = a.distro,
        cpus    = a.cpus,
        mem     = a.memory_gb,
        disk    = a.disk_gb,
        autostart = autostart_str,
        grub    = a.grub_timeout,
        pw_hash = a.pw_hash,
        vnc     = a.vnc_bind,
        keys    = keys_toml,
    )
}

fn write_defaults(config_path: &Path) -> Result<()> {
    if let Some(parent) = config_path.parent() { fs::create_dir_all(parent)?; }
    fs::write(config_path, config::sample_toml())?;
    println!("Wrote default config to {}", config_path.display());
    Ok(())
}

// ── prompt helpers ────────────────────────────────────────────────────────────

fn prompt(input: &mut impl BufRead, question: &str, default: &str) -> String {
    print!("  {question} [{default}]: ");
    io::stdout().flush().ok();
    let mut line = String::new();
    input.read_line(&mut line).ok();
    let t = line.trim().to_string();
    if t.is_empty() { default.to_string() } else { t }
}

fn prompt_bool(input: &mut impl BufRead, question: &str, default: bool) -> bool {
    let hint = if default { "Y/n" } else { "y/N" };
    print!("  {question} [{hint}]: ");
    io::stdout().flush().ok();
    let mut line = String::new();
    input.read_line(&mut line).ok();
    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no"  => false,
        _           => default,
    }
}

fn prompt_u32(input: &mut impl BufRead, question: &str, default: u32) -> u32 {
    loop {
        print!("  {question} [{default}]: ");
        io::stdout().flush().ok();
        let mut line = String::new();
        input.read_line(&mut line).ok();
        let t = line.trim().to_string();
        if t.is_empty() { return default; }
        match t.parse::<u32>() {
            Ok(n) => return n,
            Err(_) => println!("  Please enter a number."),
        }
    }
}

fn collect_ssh_keys(input: &mut impl BufRead) -> Vec<String> {
    let mut keys = Vec::new();
    let mut n = 1u32;
    loop {
        print!("  Key {n} (empty to finish): ");
        io::stdout().flush().ok();
        let mut line = String::new();
        input.read_line(&mut line).ok();
        let t = line.trim().to_string();
        if t.is_empty() { break; }
        keys.push(t);
        n += 1;
    }
    keys
}
