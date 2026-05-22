use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;
use qvm::commands;
use qvm::config::Config;
use qvm::error::Result;
use qvm::util;
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

    /// Subcommand (omit to launch the interactive TUI).
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
#[allow(clippy::enum_variant_names)]
enum Cmd {
    /// First-run setup: launches the TUI onboarding wizard. Writes
    /// /etc/qvm/config.toml and prepares dirs.
    Init {
        /// Non-interactive: skip the wizard and write default config.
        #[arg(long, short = 'y')]
        yes: bool,
        /// Overwrite an existing config file.
        #[arg(long)]
        force: bool,
    },

    /// Create and start a VM.
    #[command(alias = "run")]
    Create {
        /// VM name.
        name: String,
        /// Distro tag (e.g. ubuntu:24.04, debian:13). Uses default if omitted.
        distro: Option<String>,
        /// CPU count (no hyperthreading involved — each is a full CPU thread).
        #[arg(short = 'c', long)] cpus: Option<u32>,
        /// RAM in GB.
        #[arg(short = 'm', long = "memory")] memory_gb: Option<u32>,
        /// Disk in GB.
        #[arg(short = 's', long = "disk")] disk_gb: Option<u32>,
        /// Login user (required — qvm has no default).
        #[arg(short = 'u', long)] user: Option<String>,
        /// Plaintext password (required — qvm has no default).
        #[arg(short = 'p', long)] password: Option<String>,
        /// Do NOT autostart on host boot.
        #[arg(long)] no_autostart: bool,
        /// Disable nested virtualization for this VM (host-model -vmx -svm).
        /// Default is enabled (host-passthrough).
        #[arg(long)] no_nested: bool,
    },

    /// Delete a VM and all its data.
    #[command(alias = "delete")]
    Rm {
        name: String,
        /// Skip the confirmation prompt.
        #[arg(short = 'f', long)] force: bool,
    },

    /// Start one or more stopped VMs (or `--all`).
    Start   { names: Vec<String>, #[arg(long)] all: bool },
    /// Graceful shutdown of one or more VMs (or `--all`).
    Stop    { names: Vec<String>, #[arg(long)] all: bool },
    /// Reboot one or more VMs (or `--all`).
    #[command(alias = "reboot")]
    Restart { names: Vec<String>, #[arg(long)] all: bool },
    /// Force power-off one or more VMs (or `--all`).
    Kill    { names: Vec<String>, #[arg(long)] all: bool },

    /// Attach to the VM's serial console (Ctrl-] to detach).
    Console { name: String },

    /// SSH into a VM directly (resolves login user + IP, then execs ssh).
    Ssh     { name: String },

    /// Run a one-off command in a VM over SSH. Use `--` to separate qvm
    /// flags from the command, e.g. `qvm exec web01 -- uptime -p`.
    Exec {
        name: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        cmd:  Vec<String>,
    },

    /// List all VMs.
    #[command(alias = "ps")]
    Ls { #[arg(long)] json: bool },
    /// Show VM details.
    Inspect { name: String },
    /// Show VM IPv4.
    Ip      { name: String },
    /// Print a ready-to-use ssh command.
    SshCmd  { name: String },

    /// Print VNC connection info (and optionally open a viewer or browser bridge).
    Vnc {
        name: String,
        /// Try to launch a local VNC viewer (remote-viewer, vncviewer, ...).
        #[arg(long)] open: bool,
        /// Spawn a noVNC websocket bridge on port 6080 and print a browser URL.
        /// Requires `websockify` and `novnc` to be installed.
        #[arg(short = 'b', long)] browser: bool,
    },

    /// List configured distros.
    Distros { #[arg(long)] json: bool },
    /// List downloaded base images.
    Images  { #[arg(long)] json: bool },
    /// Download a distro's base image (atomic).
    Pull { distro: String },

    /// Change CPU count (reboot to apply).
    SetCpu { name: String, vcpus: u32 },
    /// Change RAM in GB (reboot to apply).
    SetRam { name: String, gb: u32 },
    /// Grow the disk (e.g. +50G or 200G).
    ResizeDisk { name: String, size: String },

    /// Reclaim disk space from VMs deleted out-of-band (e.g. via virsh).
    /// Lists every qcow2 / seed file with no matching libvirt domain.
    Cleanup {
        /// Skip the confirmation prompt.
        #[arg(short = 'f', long)] force: bool,
    },

    /// Detach a VM's qcow2 from its backing file (full-copy in place).
    /// Migration aid for VMs from the bash predecessor.
    Flatten { name: String },

    /// Manage VM snapshots (create, list, revert, rm, rotate).
    Snap {
        #[command(subcommand)]
        sub: SnapCmd,
    },

    /// Package a VM into a portable .qvm.tar archive.
    Export {
        /// VM name.
        name: String,
        /// Destination tarball path.
        out: PathBuf,
        /// Force live mode (--quiesce snapshot, no downtime).
        /// Requires qemu-guest-agent to be responsive.
        #[arg(long, conflicts_with = "stop")]
        live: bool,
        /// Force offline mode: stop the VM, convert, restart.
        #[arg(long, conflicts_with = "live")]
        stop: bool,
        /// After export, prune older tarballs in this directory.
        #[arg(long, requires = "keep")]
        rotate_dir: Option<PathBuf>,
        /// Keep N newest tarballs (used with --rotate-dir).
        #[arg(long, requires = "rotate_dir")]
        keep: Option<u32>,
    },

    /// Restore a VM from a .qvm.tar archive.
    Import {
        /// Tarball path.
        tarball: PathBuf,
        /// New VM name (overrides the name baked into the tarball).
        #[arg(long)] name: Option<String>,
        /// Bridge to attach (default: [network] bridge from config).
        #[arg(long)] bridge: Option<String>,
        /// osinfo override (default: derived from distro_hint, falls back to generic).
        #[arg(long)] osinfo: Option<String>,
        /// Skip sha256 verification of disk.qcow2.
        #[arg(long)] skip_verify: bool,
    },

    /// Check host dependencies (and optionally install them).
    Doctor {
        /// After listing what's missing, install it.
        #[arg(long)] install: bool,
        /// Non-interactive: assume yes to the install prompt.
        #[arg(long, short = 'y')] yes: bool,
    },
    /// Print shell completion script (bash | zsh | fish | elvish | powershell).
    Completions { shell: Shell },
}

#[derive(Subcommand)]
enum SnapCmd {
    /// Take a snapshot of a VM.
    Create(SnapCreateArgs),
    /// List a VM's snapshots.
    List { name: String },
    /// Revert a VM to a named snapshot.
    Revert(SnapRevertArgs),
    /// Delete a named snapshot.
    Rm { name: String, snap: String },
    /// Keep only the newest N snapshots, deleting the rest.
    Rotate {
        name: String,
        #[arg(long)] keep: u32,
    },
}

#[derive(Args)]
struct SnapCreateArgs {
    name: String,
    snap: String,
    /// Use qemu-guest-agent to flush guest filesystems before snapshotting.
    #[arg(long)]
    quiesce: bool,
}

#[derive(Args)]
struct SnapRevertArgs {
    name: String,
    snap: String,
    /// Force the VM running after revert (regardless of snapshot's recorded state).
    #[arg(long)]
    running: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // `completions` doesn't need root - it's purely a script generator.
    let needs_root = !matches!(cli.cmd, Some(Cmd::Completions { .. }));
    if needs_root && !util::is_root() {
        eprintln!("qvm: must run as root.");
        return ExitCode::from(1);
    }

    let cfg_path = cli.config.clone()
        .unwrap_or_else(|| PathBuf::from("/etc/qvm/config.toml"));

    match dispatch(&cli, &cfg_path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Stderr is the canonical place for error output. The Error:
            // prefix is colored red+bold if the parent process attached a
            // TTY; piped/redirected stderr gets plain text automatically.
            eprintln!("{} {e}", qvm::style::err("Error:"));
            ExitCode::from(1)
        }
    }
}

fn dispatch(cli: &Cli, cfg_path: &std::path::Path) -> Result<()> {
    // No subcommand → launch the interactive TUI.
    let Some(cmd) = &cli.cmd else {
        if !cfg_path.exists() {
            // First-run experience. Guides through bridge, SSH keys,
            // storage paths and (optionally) a first image pull, then
            // writes /etc/qvm/config.toml. Falls back to the old
            // text-only error if the TUI can't open (no PTY, etc).
            if let Err(e) = qvm::tui::onboard::run(cfg_path) {
                return Err(qvm::error::Error::User(format!(
                    "onboarding failed: {e}\nFall back to: sudo qvm init"
                )));
            }
        }
        let cfg = Config::load(Some(cfg_path))?;
        return qvm::tui::run(&cfg);
    };

    // Commands that don't need a config file.
    match cmd {
        Cmd::Init { yes, force }       => return commands::init::run(cfg_path, *yes, *force),
        Cmd::Doctor { install, yes }   => return commands::doctor::run_doctor(*install, *yes),
        Cmd::Completions { shell }     => return commands::completions::run::<Cli>(*shell),
        _ => {}
    }

    let cfg = Config::load(Some(cfg_path))?;

    match cmd {
        Cmd::Init { .. } | Cmd::Doctor { .. } | Cmd::Completions { .. } => unreachable!("handled above"),

        Cmd::Create { name, distro, cpus, memory_gb, disk_gb, user, password, no_autostart, no_nested } => {
            commands::create::run(&cfg, commands::create::Args {
                name: name.clone(),
                distro: distro.clone(),
                cpus: *cpus,
                memory_gb: *memory_gb,
                disk_gb: *disk_gb,
                user: user.clone(),
                password: password.clone(),
                no_autostart: *no_autostart,
                nested: if *no_nested { Some(false) } else { None },
            })
        }

        Cmd::Rm { name, force }   => commands::delete::run(&cfg, name, *force),
        Cmd::Start   { names, all } => commands::lifecycle::batch(commands::lifecycle::Verb::Start,   names, *all),
        Cmd::Stop    { names, all } => commands::lifecycle::batch(commands::lifecycle::Verb::Stop,    names, *all),
        Cmd::Restart { names, all } => commands::lifecycle::batch(commands::lifecycle::Verb::Restart, names, *all),
        Cmd::Kill    { names, all } => commands::lifecycle::batch(commands::lifecycle::Verb::Kill,    names, *all),
        Cmd::Console { name }     => commands::console::run(name),
        Cmd::Ssh     { name }     => commands::info::ssh_exec(&cfg, name),
        Cmd::Exec { name, cmd }   => commands::info::ssh_exec_cmd(&cfg, name, cmd),

        Cmd::Ls { json }          => commands::info::ls(*json),
        Cmd::Inspect { name }     => commands::info::inspect(name),
        Cmd::Ip      { name }     => commands::info::ip(name),
        Cmd::SshCmd  { name }     => commands::info::ssh_cmd(&cfg, name),

        Cmd::Vnc { name, open, browser } => commands::vnc::run(&cfg, name, *open, *browser),

        Cmd::Distros { json }     => commands::images::distros(&cfg, *json),
        Cmd::Images  { json }     => commands::images::images(&cfg, *json),
        Cmd::Pull { distro }      => commands::pull::run(&cfg, distro),

        Cmd::SetCpu { name, vcpus } => commands::resources::set_cpu(name, *vcpus),
        Cmd::SetRam { name, gb }    => commands::resources::set_ram(name, *gb),
        Cmd::ResizeDisk { name, size } => commands::resources::resize_disk(&cfg, name, size),

        Cmd::Cleanup { force }      => commands::cleanup::run(&cfg, *force),
        Cmd::Flatten { name }       => commands::flatten::run(&cfg, name),

        Cmd::Snap { sub }           => match sub {
            SnapCmd::Create(a)            => commands::snap::create(&a.name, &a.snap, a.quiesce),
            SnapCmd::List   { name }      => commands::snap::list(name),
            SnapCmd::Revert(a)            => commands::snap::revert(&a.name, &a.snap, a.running),
            SnapCmd::Rm     { name, snap }=> commands::snap::remove(name, snap),
            SnapCmd::Rotate { name, keep }=> commands::snap::rotate(name, *keep),
        },

        Cmd::Export { name, out, live, stop, rotate_dir, keep } => {
            let mode = if *live { commands::export::Mode::Live }
                       else if *stop { commands::export::Mode::Stop }
                       else { commands::export::Mode::Auto };
            commands::export::run(&cfg, commands::export::Args {
                name: name.clone(),
                out:  out.clone(),
                mode,
                rotate_dir: rotate_dir.clone(),
                keep: *keep,
            })
        }
        Cmd::Import { tarball, name, bridge, osinfo, skip_verify } => {
            commands::import::run(&cfg, commands::import::Args {
                tarball: tarball.clone(),
                name:    name.clone(),
                bridge:  bridge.clone(),
                osinfo:  osinfo.clone(),
                skip_verify: *skip_verify,
            })
        }
    }
}
