use crate::cloudinit::Seed;
use crate::cmd::{require, run as cmd_run, run_inherit};
use crate::config::Config;
use crate::error::{Error, Result};
use crate::libvirt;
use crate::util::{hash_password, require_username};

#[derive(Debug)]
pub struct Args {
    pub name:     String,
    pub distro:   Option<String>,
    pub cpus:     Option<u32>,
    pub memory_gb:Option<u32>,
    pub disk_gb:  Option<u32>,
    pub user:     Option<String>,
    pub password: Option<String>,
    pub no_autostart: bool,
    /// `Some(true)` forces nested virt on, `Some(false)` forces off,
    /// `None` falls back to `cfg.defaults.nested`. CLI maps `--no-nested`
    /// to `Some(false)`.
    pub nested:   Option<bool>,
}

pub fn run(cfg: &Config, a: Args) -> Result<()> {
    // Precondition: domain must not already be defined.
    libvirt::require_absent(&a.name)?;
    require("virt-install")?;
    require("qemu-img")?;
    require("genisoimage")?;

    // --- resolve params ---
    let name   = a.name;
    let distro = a.distro.unwrap_or_else(|| cfg.defaults.distro.clone());
    let cpus   = a.cpus.unwrap_or(cfg.defaults.cpus);
    let ram_gb = a.memory_gb.unwrap_or(cfg.defaults.memory_gb);
    let disk_gb= a.disk_gb.unwrap_or(cfg.defaults.disk_gb);

    // Username and password are NEVER defaulted. The user must supply both
    // every time. The reason: an untyped default that the operator forgets
    // to set ends up baked into every cloud-init seed in the homelab — a
    // sleeper credential. Better to refuse to create the VM than to ship
    // one with implicit credentials.
    let user = a.user.ok_or_else(|| Error::User(
        "--user is required. Pass -u <name> (or set the VM's login user in \
         the TUI Create form).".into()
    ))?;
    require_username(&user)?;

    let pw_plain = a.password.ok_or_else(|| Error::User(
        "--password is required. Pass -p <password> (or set it in the TUI \
         Create form). qvm intentionally has no default password.".into()
    ))?;
    let pw_hash = hash_password(&pw_plain)?;

    if cpus == 0 || ram_gb == 0 || disk_gb == 0 {
        return Err(Error::User("cpus, memory, and disk must all be > 0".into()));
    }

    let d = cfg.distro(&distro)?;
    // Create the qvm dirs FIRST — auto-pull writes to <images>/X.partial
    // and would otherwise fail when migrating from a deleted dir layout.
    cfg.ensure_dirs()?;

    let base = cfg.image_path(&distro)?;
    if !base.exists() {
        // docker-style: pull on demand instead of forcing the user back to
        // `qvm pull`. `pull_one` writes atomically (`.partial` → rename) and
        // inherits wget's progress bar, so the user sees download progress.
        println!("Unable to find image '{distro}' locally, pulling...");
        crate::commands::pull::pull_one(cfg, &distro)?;
    }
    let disk_path = cfg.vm_disk(&name);
    let iso_path  = cfg.vm_seed_iso(&name);
    let ci_dir    = cfg.vm_ci_dir(&name);

    // --- cloud-init seed ---
    println!("Generating cloud-init seed...");
    Seed {
        vm_name: &name,
        login_user: &user,
        login_shell: &d.shell,
        password_hash: &pw_hash,
        ssh_keys: &cfg.ssh_keys,
        grub_timeout: cfg.defaults.grub_timeout,
    }.build(&ci_dir, &iso_path)?;

    // --- SELF-CONTAINED disk: copy the base, do NOT chain it. ---
    //
    // This is the architectural fix that prevents the Dev/Hermes class
    // of corruption. Pulling new bases later cannot affect existing VMs.
    let (image_name, _) = d.variant_for(crate::arch::host())?;
    println!("Creating {disk_gb}G self-contained disk from {}...", image_name);
    run_inherit("qemu-img", [
        "convert", "-p", "-O", "qcow2",
        base.to_str().unwrap(),
        disk_path.to_str().unwrap(),
    ])?;
    cmd_run("qemu-img", [
        "resize", "-q",
        disk_path.to_str().unwrap(),
        &format!("{disk_gb}G"),
    ])?;

    // --- define + start via virt-install --import ---
    println!("Defining and starting VM...");
    let memory_mb = (ram_gb as u64) * 1024;
    let cpus_str  = cpus.to_string();
    let memory_str= memory_mb.to_string();
    let osinfo    = format!("name={},require=off", d.osinfo);
    let netarg    = format!("bridge={},model=virtio", cfg.network.bridge);
    let diskarg   = format!("path={},format=qcow2,bus=virtio", disk_path.display());
    let cdromarg  = format!("path={},device=cdrom", iso_path.display());
    let vncarg    = format!("vnc,listen={}", cfg.vnc.bind);

    // Nested-virt knob.
    //   true (default): host-passthrough exposes every host CPU flag to the
    //                   guest, including vmx/svm — so the guest can run KVM
    //                   inside itself.
    //   false:          host-model + explicitly subtract vmx/svm so a guest
    //                   never accidentally inherits nested-virt extensions.
    //                   Useful when you're handing a VM to someone you don't
    //                   want spinning up VMs of their own.
    let nested = a.nested.unwrap_or(cfg.defaults.nested);
    let cpu_arg: String = if nested {
        "host-passthrough".into()
    } else {
        "host-model,-vmx,-svm".into()
    };

    let mut args: Vec<String> = vec![
        "--name".into(),       name.clone(),
        "--memory".into(),     memory_str,
        "--vcpus".into(),      cpus_str,
        "--cpu".into(),        cpu_arg,
        "--disk".into(),       diskarg,
        "--disk".into(),       cdromarg,
        "--osinfo".into(),     osinfo,
        "--graphics".into(),   vncarg,
        "--network".into(),    netarg,
        "--channel".into(),    "unix,target_type=virtio,name=org.qemu.guest_agent.0".into(),
        // Explicit virtio memory balloon. virt-install adds one by default
        // for most osinfo IDs, but that defaulting can change between
        // releases; spelling it out keeps every qvm-created VM consistent.
        // Guest needs qemu-guest-agent (cloud-init enables it) for the host
        // to actually reclaim memory — until then the device sits idle.
        "--memballoon".into(), "model=virtio".into(),
        "--import".into(),
        "--noautoconsole".into(),
    ];
    // Arch-specific knobs:
    //   - amd64 + UEFI distro (e.g. Alpine): --machine q35 --boot uefi…
    //   - arm64: --arch aarch64 --machine virt --boot uefi,…  (mandatory)
    if crate::arch::is_arm() {
        args.push("--arch".into());    args.push("aarch64".into());
        args.push("--machine".into()); args.push("virt".into());
        args.push("--boot".into());    args.push("uefi,loader.secure=no".into());
    } else if d.uefi {
        args.push("--machine".into()); args.push("q35".into());
        args.push("--boot".into());    args.push("uefi,loader.secure=no".into());
    }
    run_inherit("virt-install", args.iter().map(|s| s.as_str()))?;

    let autostart = !a.no_autostart && cfg.defaults.autostart;
    if autostart { libvirt::autostart_on(&name)?; }

    // --- summary ---
    use crate::style as s;
    println!();
    println!("{} {}", s::ok("✓"), s::ok(format!("VM '{name}' created")));
    println!(
        "  {} {distro}   {} {cpus}   {} {ram_gb}G   {} {disk_gb}G   {} {user}",
        s::label("distro"),
        s::label("cpus"),
        s::label("ram"),
        s::label("disk"),
        s::label("user"),
    );
    if !nested {
        println!("  {} {}", s::label("nested virtualization:"), s::warn("disabled"));
    }
    println!();
    println!("  {} {name}        {}", s::cmd("qvm ip"),      s::dim("# address (wait ~30s for boot)"));
    println!("  {} {name}   {}",      s::cmd("qvm ssh-cmd"), s::dim("# ssh command"));
    Ok(())
}
