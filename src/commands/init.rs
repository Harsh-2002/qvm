use crate::config;
use crate::error::{Error, Result};
use crate::tui;
use std::fs;
use std::path::Path;

/// `qvm init` — first-run setup.
///
/// Defaults to the TUI onboarding wizard (same UX as bare `qvm` on a fresh
/// host). `--yes` writes a default config without prompting, for automation.
/// `--force` overwrites an existing config.
pub fn run(config_path: &Path, yes: bool, force: bool) -> Result<()> {
    if config_path.exists() && !force {
        return Err(Error::User(format!(
            "config already exists at {}\n  - edit it directly, or\n  - rerun with --force to redo setup, or\n  - delete the file and rerun `qvm init`.",
            config_path.display()
        )));
    }

    if yes {
        write_defaults(config_path)?;
    } else if let Err(e) = tui::onboard::run(config_path) {
        return Err(Error::User(format!(
            "onboarding failed: {e}\n  - rerun with --yes to write defaults non-interactively."
        )));
    }

    // Whether the TUI or --yes wrote the config, ensure dirs are present
    // so the very next `qvm run` doesn't trip on a missing /var/lib/qvm.
    let cfg = config::Config::load(Some(config_path))?;
    cfg.ensure_dirs()?;
    Ok(())
}

fn write_defaults(config_path: &Path) -> Result<()> {
    if let Some(parent) = config_path.parent() { fs::create_dir_all(parent)?; }
    fs::write(config_path, config::sample_yaml())?;
    println!("Wrote default config to {}", config_path.display());
    Ok(())
}

// ── shared by TUI onboarding (`src/tui/onboard.rs`) ──────────────────────────

pub struct WizardAnswers<'a> {
    pub bridge:         &'a str,
    pub distro:         &'a str,
    pub cpus:           u32,
    pub memory_gb:      u32,
    pub disk_gb:        u32,
    pub autostart:      bool,
    pub grub_timeout:   u32,
    pub vnc_bind:       &'a str,
    pub ssh_keys:       &'a [String],
    pub images_path:    &'a str,
    pub vms_path:       &'a str,
    pub cloudinit_path: &'a str,
}

/// Render the final `/etc/qvm/config.yml`. Exposed so the TUI onboarding
/// wizard (`src/tui/onboard.rs`) writes the same YAML as `qvm init`.
///
/// Hand-written rather than `serde_yaml::to_string` so the layout stays
/// predictable: aligned columns, no surprise quoting, minimal comments.
pub fn render_config(a: WizardAnswers<'_>) -> String {
    let keys_yaml = if a.ssh_keys.is_empty() {
        "ssh_keys: []".to_string()
    } else {
        let mut s = String::from("ssh_keys:\n");
        for k in a.ssh_keys {
            s.push_str(&format!("  - {}\n", yaml_inline(k)));
        }
        s.trim_end().to_string()
    };

    let autostart = if a.autostart { "true" } else { "false" };

    format!(
"# qvm config. Every key is optional — omit any line to keep the default.

{keys}

paths:
  images:    {images}
  vms:       {vms}
  cloudinit: {ci}

network:
  bridge: {bridge}

defaults:
  distro:       {distro}
  cpus:         {cpus}
  memory_gb:    {mem}
  disk_gb:      {disk}
  autostart:    {autostart}
  grub_timeout: {grub}

vnc:
  bind: {vnc}

tui:
  theme: mocha             # mocha | latte

motd:
  enable: true
  color:  auto             # auto | always | never
",
        keys      = keys_yaml,
        images    = yaml_inline(a.images_path),
        vms       = yaml_inline(a.vms_path),
        ci        = yaml_inline(a.cloudinit_path),
        bridge    = yaml_inline(a.bridge),
        distro    = yaml_inline(a.distro),
        cpus      = a.cpus,
        mem       = a.memory_gb,
        disk      = a.disk_gb,
        autostart = autostart,
        grub      = a.grub_timeout,
        vnc       = yaml_inline(a.vnc_bind),
    )
}

/// Render a scalar as a YAML inline string. We single-quote anything that
/// could confuse the parser (colons, leading `*`/`&`/`-`, etc.) or contains
/// whitespace at boundaries. Strings of strict `[A-Za-z0-9_.-]` are emitted
/// bare for readability — these are the common case (paths, distro keys).
pub fn yaml_inline(s: &str) -> String {
    fn safe_bare(s: &str) -> bool {
        if s.is_empty() { return false; }
        // Conservative: only emit bare if every char is one of these.
        // Drops colons, spaces, brackets, anything YAML might misread.
        s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':' | ' '))
            && !s.starts_with(' ') && !s.ends_with(' ')
            // Reserved words YAML would parse as booleans.
            && !matches!(s.to_ascii_lowercase().as_str(),
                "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "~")
            // Anything that looks like a YAML flow-collection or anchor.
            && !s.starts_with(['*', '&', '!', '?', '|', '>', '\'', '"', '%', '@', '`'])
            // A leading dash followed by space is a YAML sequence indicator.
            && !s.starts_with("- ")
    }
    if safe_bare(s) {
        s.to_string()
    } else {
        // Single-quote, doubling any embedded `'`.
        format!("'{}'", s.replace('\'', "''"))
    }
}
