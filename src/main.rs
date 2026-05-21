use clap::{Parser, Subcommand};
use clap_complete::Shell;
use qvm::commands;
use qvm::config::Config;
use qvm::error::Result;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "qvm",
    version,
    about = "Thin, opinionated CLI for managing KVM/libvirt VMs.",
    long_about = None,
)]
struct Cli {
    /// Path to config file (defaults to /etc/qvm/config.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
#[allow(clippy::enum_variant_names)]
enum Cmd {
    /// First-run setup: write /etc/qvm/config.toml and prepare dirs.
    Init {
        /// After setup, download all baseline images.
        #[arg(long)]
        pull_all: bool,
        /// Non-interactive: skip the wizard and write defaults immediately.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Create and start a VM.
    #[command(alias = "run")]
    Create {
        /// VM name.
        name: String,
        /// Distro tag (e.g. ubuntu:24.04, debian:13). Uses default if omitted.
        distro: Option<String>,
        /// vCPUs.
        #[arg(short = 'c', long)] cpus: Option<u32>,
        /// RAM in GB.
        #[arg(short = 'm', long = "memory")] memory_gb: Option<u32>,
        /// Disk in GB.
        #[arg(short = 's', long = "disk")] disk_gb: Option<u32>,
        /// Login user (default: random vmXXXXXX).
        #[arg(short = 'u', long)] user: Option<String>,
        /// Plaintext password (default: configured hash).
        #[arg(short = 'p', long)] password: Option<String>,
        /// Do NOT autostart on host boot.
        #[arg(long)] no_autostart: bool,
    },

    /// Delete a VM and all its data.
    #[command(alias = "delete")]
    Rm {
        name: String,
        /// Skip the confirmation prompt.
        #[arg(short = 'f', long)] force: bool,
    },

    /// Start a stopped VM.
    Start   { name: String },
    /// Graceful shutdown.
    Stop    { name: String },
    /// Reboot.
    #[command(alias = "reboot")]
    Restart { name: String },
    /// Force power-off.
    Kill    { name: String },

    /// List all VMs.
    #[command(alias = "ps")]
    Ls,
    /// Show VM details.
    Inspect { name: String },
    /// Show VM IPv4.
    Ip      { name: String },
    /// Print a ready-to-use ssh command.
    SshCmd  { name: String },

    /// Print VNC connection info (and optionally open a viewer).
    Vnc {
        name: String,
        #[arg(long)] open: bool,
    },

    /// List configured distros.
    Distros,
    /// List downloaded base images.
    Images,
    /// Download a distro's base image (atomic).
    Pull { distro: String },

    /// Change vCPU count (reboot to apply).
    SetCpu { name: String, vcpus: u32 },
    /// Change RAM in GB (reboot to apply).
    SetRam { name: String, gb: u32 },
    /// Grow the disk (e.g. +50G or 200G).
    ResizeDisk { name: String, size: String },

    /// Check host dependencies (and optionally install them).
    Doctor {
        /// After listing what's missing, install it.
        #[arg(long)] install: bool,
    },
    /// Print shell completion script (bash | zsh | fish | elvish | powershell).
    Completions { shell: Shell },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // `completions` and `doctor` don't need root - they're diagnostic.
    let needs_root = !matches!(cli.cmd, Cmd::Completions { .. });
    if needs_root && !is_root() {
        eprintln!("qvm: must run as root.");
        return ExitCode::from(1);
    }

    let cfg_path = cli.config.clone()
        .unwrap_or_else(|| PathBuf::from("/etc/qvm/config.toml"));

    match dispatch(&cli, &cfg_path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => { eprintln!("Error: {e}"); ExitCode::from(1) }
    }
}

fn dispatch(cli: &Cli, cfg_path: &std::path::Path) -> Result<()> {
    // Commands that don't need a config file.
    match &cli.cmd {
        Cmd::Init { pull_all, yes } => return commands::init::run(cfg_path, *pull_all, *yes),
        Cmd::Doctor { install } => return commands::doctor::run_doctor(*install),
        Cmd::Completions { shell } => return commands::completions::run::<Cli>(*shell),
        _ => {}
    }

    let cfg = Config::load(Some(cfg_path))?;
    cfg.ensure_dirs()?;

    match &cli.cmd {
        Cmd::Init { .. } | Cmd::Doctor { .. } | Cmd::Completions { .. } => unreachable!("handled above"),

        Cmd::Create { name, distro, cpus, memory_gb, disk_gb, user, password, no_autostart } => {
            commands::create::run(&cfg, commands::create::Args {
                name: name.clone(),
                distro: distro.clone(),
                cpus: *cpus,
                memory_gb: *memory_gb,
                disk_gb: *disk_gb,
                user: user.clone(),
                password: password.clone(),
                no_autostart: *no_autostart,
            })
        }

        Cmd::Rm { name, force }   => commands::delete::run(&cfg, name, *force),
        Cmd::Start   { name }     => commands::lifecycle::start(name),
        Cmd::Stop    { name }     => commands::lifecycle::stop(name),
        Cmd::Restart { name }     => commands::lifecycle::restart(name),
        Cmd::Kill    { name }     => commands::lifecycle::kill(name),

        Cmd::Ls                   => commands::info::ls(),
        Cmd::Inspect { name }     => commands::info::inspect(name),
        Cmd::Ip      { name }     => commands::info::ip(name),
        Cmd::SshCmd  { name }     => commands::info::ssh_cmd(&cfg, name),

        Cmd::Vnc { name, open }   => commands::vnc::run(&cfg, name, *open),

        Cmd::Distros              => commands::images::distros(&cfg),
        Cmd::Images               => commands::images::images(&cfg),
        Cmd::Pull { distro }      => commands::pull::run(&cfg, distro),

        Cmd::SetCpu { name, vcpus } => commands::resources::set_cpu(name, *vcpus),
        Cmd::SetRam { name, gb }    => commands::resources::set_ram(name, *gb),
        Cmd::ResizeDisk { name, size } => commands::resources::resize_disk(&cfg, name, size),
    }
}

#[cfg(unix)]
fn is_root() -> bool {
    // SAFETY: geteuid() is always safe to call.
    unsafe { libc_geteuid() == 0 }
}

extern "C" { fn geteuid() -> u32; }
unsafe fn libc_geteuid() -> u32 { geteuid() }
