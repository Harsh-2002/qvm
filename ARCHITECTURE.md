# qvm — architecture

This is the low-level design document for `qvm`. It's deeper than the
README (user-facing) and deeper than `CLAUDE.md` (contributor cheatsheet).
If you want to understand *why* qvm is shaped the way it is — read this.

Three top-level invariants drive almost every decision:

1. **Every VM disk is a self-contained copy.** No qcow2 backing files,
   ever. ([§ 2](#2-the-disaster-that-birthed-this-tool))
2. **libvirt is the source of truth.** No parallel state file.
   ([§ 3](#3-architectural-invariants))
3. **Distro differences are data, not code.** A registry of records, plus
   a single distro-agnostic first-boot script. Adding a new distro is a
   TOML edit, not a code change. ([§ 8](#8-distro-registry-data-driven))

The rest of this document expands on those and shows how every layer of
the codebase enforces them.

---

## Table of contents

1. [Mission and non-goals](#1-mission-and-non-goals)
2. [The disaster that birthed this tool](#2-the-disaster-that-birthed-this-tool)
3. [Architectural invariants](#3-architectural-invariants)
4. [Surface area: CLI, TUI, VNC bridge](#4-surface-area-cli-tui-vnc-bridge)
5. [Module map](#5-module-map)
6. [Lifecycle of a VM](#6-lifecycle-of-a-vm)
7. [Cloud-init pipeline](#7-cloud-init-pipeline)
8. [Distro registry (data-driven)](#8-distro-registry-data-driven)
9. [The TUI internals](#9-the-tui-internals)
10. [The VNC console paths](#10-the-vnc-console-paths)
11. [Error model](#11-error-model)
12. [Extension points](#12-extension-points)
13. [What qvm deliberately doesn't have](#13-what-qvm-deliberately-doesnt-have)
14. [Build, release, install](#14-build-release-install)
15. [Test strategy](#15-test-strategy)
16. [Open work](#16-open-work)

---

## 1. Mission and non-goals

qvm is a thin, opinionated CLI + TUI for managing KVM/libvirt VMs on a
single host. The audience is a homelab operator with one server (or two,
managed independently), who has historically used some combination of
`virt-install`, `virsh`, custom shell scripts, or Cockpit, and is unhappy
with all of them.

**In scope:**
- Create / start / stop / restart / kill / delete VMs
- List, inspect, get IP, generate SSH command
- Pull and list distro base images (5 baked in, more via config)
- VNC connection info (replacing the need for Cockpit for graphics)
- Browser-based VNC console via on-demand `websockify` + noVNC
  (`qvm vnc --browser`, plus the TUI's `b` key, with an inline QR for
  mobile scanning)
- Resource changes: CPU, RAM, disk grow
- Interactive TUI (`qvm` with no subcommand) — Proxmox-style sidebar
  + content pane, theming, **keyboard-only** (mouse capture removed
  intentionally; see § 9.5)
- One-time setup: `qvm init` (interactive TUI wizard)
- A single static binary per arch (`amd64` + `arm64`) — no daemon,
  no extra runtime

**Out of scope, by design:**
- Multi-host / clustering / federation
- Live migration
- Storage pools / LVM / Ceph / anything other than qcow2 files in a
  directory
- A persistent web admin UI (we tried it, removed it — see [§ 13](#13-what-qvm-deliberately-doesnt-have))
- Authentication, TLS, multi-tenancy
- Image building / Packer-style workflows
- Container support — this is a VM tool
- Automatic mirror selection / geo-pinning
- A daemon. There is no daemon. libvirtd is the daemon.

If you want one of those, fork qvm. A small tool that does its job is
more valuable than a Swiss-Army knife.

---

## 2. The disaster that birthed this tool

qvm exists because the previous tool — a bash script called `vm` — almost
caused a multi-VM data-loss incident on the author's homelab.

That script created each VM as a thin qcow2 **overlay** on top of a
shared base image:

```
qemu-img create -f qcow2 -b /SSD/Images/ubuntu.img new-vm.qcow2 200G
```

Overlay disks only store changed blocks; unchanged blocks (kernel,
`/lib`, `/usr`) are read at boot time from the backing file.

Months later, the operator re-ran `vm pull ubuntu24`, which downloaded a
newer Ubuntu daily build over the existing base file. Every VM that
depended on the old base now read **a different Ubuntu** for any block
it hadn't written. Symptoms:

- GRUB rescue prompt on boot ("invalid arch-independent ELF magic")
- After forcing a direct kernel boot, `/sbin/init` failed to load
  shared libraries (`/lib` was partial garbage from the new base)
- Config files like `/etc/mdadm/mdadm.conf` contained literal binary
  noise
- libguestfs auto-inspection refused to mount because it couldn't
  parse augeas configs

Two production VMs were nearly lost. Recovery required mounting via
`qemu-nbd`, extracting user data with rsync, and rebuilding from
scratch.

**The root cause was architectural, not a code bug.** Any tool that
creates overlay-on-shared-base VMs has this disaster mode, and there is
no amount of "be careful when you pull" warnings that protects against
it.

Hence rule #1 of qvm: **every VM disk is a full, self-contained copy.
There are no backing files. Ever.**

This costs ~10–30 seconds and ~500 MB – 1 GB of disk per VM at create
time. That trade is worth making once, forever, instead of explaining
"don't run pull while VMs exist" to someone (including future-you) at
2 AM.

---

## 3. Architectural invariants

Five hard rules the code obeys. They are reflected in tests, in module
boundaries, and in the way commits get reviewed.

### 3.1 Every VM disk is a full copy

See [§ 2](#2-the-disaster-that-birthed-this-tool) for the why. Mechanically:

- `src/commands/create.rs` is the only place that creates a VM disk.
- It does so via `qemu-img convert -O qcow2 <base> <vm>` followed by
  `qemu-img resize -q <vm> <size>G`.
- The `-b` flag is **never** present in any `qemu-img create` call
  anywhere in the source. A grep of the source for `qemu-img.*-b` returns
  zero results. The integration test `integration/05-self-contained.sh`
  enforces this.

A new VM disk holds the full base contents. From that moment, it is
independent of the base file; running `qvm pull` to refresh the base
image affects only *future* VMs.

### 3.2 libvirt is the source of truth

qvm does not maintain a parallel database of "which VMs exist". The
answer is whatever `virsh list --all` says. Specifically:

- `qvm ls` is `virsh list --all` plus light formatting.
- `qvm inspect` is `virsh dominfo`.
- The TUI's sidebar list is built from `libvirt::domains()` which calls
  `virsh list --all --name` and `virsh domstate`.
- The exception: a tiny per-VM sidecar `<cloudinit_dir>/<name>/.vmuser`
  containing the login user, so `qvm ssh-cmd` can print the right
  username. That's it.

This rule is what makes qvm safe to install, uninstall, or replace
with another tool. There's no extra state to migrate.

### 3.3 No per-distro code branches

If your code mentions a distro name in a conditional, it's wrong.

- Distro data (image filename, osinfo id, login shell, UEFI flag,
  download URL) lives in `src/config.rs::builtin_distros()` plus
  user-added entries in `/etc/qvm/config.yml`.
- Distro behaviour differences (systemd vs OpenRC, update-grub vs
  grub2-mkconfig, apt vs dnf vs apk) are resolved **inside the guest**
  by a generic first-boot script. The script feature-detects with
  `command -v` and `[ -f ]` and never branches on `/etc/os-release`.

See [§ 7](#7-cloud-init-pipeline) for the script itself.

The payoff: adding a sixth distro is one TOML stanza. No code review,
no testing matrix.

### 3.4 Shell out, don't link

qvm has zero FFI to libvirt-rs, no qemu C bindings. Every external
interaction goes through `cmd::run` / `cmd::run_inherit` / `cmd::run_tty`
in `src/cmd.rs`.

Consequences:

- Debugging is trivial. Print the exact command, copy it into a shell,
  reproduce.
- The tool works on any host where the system tools work, no library
  version dance.
- The binary stays tiny (~2 MB stripped musl) because we don't link in
  half of qemu.
- Strace works. So does `set -x` thinking — the code reads like a
  shell pipeline.

### 3.5 Stable URLs, not dailies, for built-in distros

Even with rule 3.1 — because re-rolling the base only affects new
VMs, not existing ones — we still prefer URLs that point at stable
release channels rather than nightlies. The base URL for each built-in
distro is reviewed when adding it:

| Distro            | URL pattern                                                  | Stability                          |
|-------------------|--------------------------------------------------------------|------------------------------------|
| ubuntu:24.04      | `releases/noble/release/ubuntu-24.04-server-...img`          | latest stable point release        |
| ubuntu:26.04      | `releases/26.04/release/ubuntu-26.04-server-...img`          | latest stable point release        |
| debian:13         | `cloud/trixie/latest/debian-13-genericcloud-...qcow2`        | latest stable trixie point release |
| fedora:42         | `releases/42/Cloud/.../Fedora-Cloud-Base-Generic-42-1.1`     | pinned GA                          |
| alpine:3.20       | `releases/cloud/...alpine-3.20.3-x86_64-uefi-r0.qcow2`       | pinned point release               |
| rocky:9           | `images/x86_64/Rocky-9-GenericCloud-Base.latest.x86_64`      | latest stable point release        |
| almalinux:9       | `cloud/x86_64/images/AlmaLinux-9-GenericCloud-latest.x86_64` | latest stable point release        |
| opensuse:15.6     | `appliances/openSUSE-Leap-15.6-Minimal-VM...Cloud-Build19.146`| pinned build                       |
| centos-stream:10  | `10-stream/x86_64/images/CentOS-Stream-GenericCloud-10-latest`| latest stable point release        |
| arch              | `images/latest/Arch-Linux-x86_64-cloudimg.qcow2`             | rolling (x86_64 only — see note)  |

Note: `arch` is a deliberate exception. Arch upstream publishes only an
x86_64 cloud image and points `/latest/` at the most recent snapshot
(rolling, no pinned point releases). Arch users expect this; the rest of
the registry follows the stable-URL rule.

---

## 4. Surface area: CLI, TUI, VNC bridge

qvm exposes three orthogonal frontends over one shared library of
command functions. Every action a frontend takes is a call into
`commands::*` — there is no parallel state, no duplicated logic.

```
                      ┌───────────────────────────────────────┐
                      │              qvm  binary              │
                      └───────────────────┬───────────────────┘
                                          │
              ┌───────────────────────────┼───────────────────────────┐
              ▼                           ▼                           ▼
    ╭───────────────────╮       ╭───────────────────╮       ╭───────────────────╮
    │        CLI        │       │        TUI        │       │    VNC bridge     │
    │                   │       │                   │       │                   │
    │  scripting,       │       │  primary admin    │       │  qvm vnc          │
    │  idempotent,      │       │  experience       │       │       --browser   │
    │  cron / CI        │       │  Proxmox-style    │       │  · TUI 'b' key    │
    ╰─────────┬─────────╯       ╰─────────┬─────────╯       ╰─────────┬─────────╯
              │                           │                           │
              └───────────────────────────┼───────────────────────────┘
                                          ▼
                      ┌───────────────────────────────────────┐
                      │           commands::*   (lib)         │
                      │                                       │
                      │   create  ·  delete  ·  lifecycle     │
                      │   pull    ·  info    ·  vnc  ·  init  │
                      └───────────────────┬───────────────────┘
                                          ▼
                      ┌───────────────────────────────────────┐
                      │         libvirt::*   +   cmd::*       │
                      │                                       │
                      │   Every external interaction is a     │
                      │   virsh / virt-install / qemu-img     │
                      │   shell-out. No FFI, no bindings.     │
                      └───────────────────┬───────────────────┘
                                          ▼
                                ╭───────────────────╮
                                │      libvirtd     │
                                │   (system svc)    │
                                ╰─────────┬─────────╯
                                          ▼
                                ╭───────────────────╮
                                │     QEMU / KVM    │
                                ╰───────────────────╯
```

Critical rule: **the TUI and the VNC bridge call into `commands::*`
exactly the same way the CLI does.** There is no parallel "create logic"
for the TUI vs the CLI. When the TUI's create modal submits, it calls
`commands::create::run(&cfg, args)` — the same function the CLI
dispatches to.

This is why the project has stayed small (~3 kLOC) despite supporting
multiple frontends. It is also why removing the experimental web admin
UI was cheap: the management logic was already shared.

The three surfaces:

- **CLI** (`src/main.rs`, clap-defined): the source of truth for the
  command surface. Stable, scriptable, idempotent where it matters
  (e.g., `qvm doctor --install --yes`). What users put in cron / Ansible /
  `Makefile`.

- **TUI** (`src/tui/`): launched by running `qvm` with no subcommand.
  Catppuccin Mocha theme by default (or Latte light, via `[tui] theme`).
  Sidebar lists VMs; right pane shows selected VM details. All actions
  are visible as labeled keybindings in a bottom action bar.
  **Keyboard-only** (mouse capture removed — see § 9.5).

- **VNC bridge** (`src/commands/vnc.rs`): when invoked with `--browser`,
  qvm spawns `websockify` + serves the system-installed noVNC bundle on
  TCP port 6080. Prints the connect URL and a Unicode-block QR code (so
  you can scan it from a phone). When the user Ctrl-C's, the bridge
  exits. There is no daemon mode.

---

## 5. Module map

```
src/
├── main.rs                CLI dispatch (clap). Trivial — all logic lives below.
├── lib.rs                 Library surface so the test suite can exercise modules.
├── error.rs               One Result type. Two variants. No anyhow soup.
├── cmd.rs                 The only layer that touches external processes.
│                          Three variants: run (collect stdout), run_inherit
│                          (pipe to TTY, null stdin — wget/qemu-img), and
│                          run_tty (full TTY inheritance — virsh console).
├── libvirt.rs             Thin virsh wrapper. exists/is_running/start/stop/...
│                          vnc_endpoint() returns both display number and TCP
│                          port (the original bug was confusing the two).
│                          ipv4() probes via guest agent → DHCP lease → ARP.
│                          domains() returns a structured Vec<Domain> for the TUI.
├── config.rs              TOML schema, baked-in defaults, distro registry.
│                          5 built-in distros; user config layers on top.
├── cloudinit.rs           Seed generator. write_files() then build_iso() — split
│                          so tests don't need genisoimage installed. Emits a
│                          generic first-boot script (no per-distro branches).
├── util.rs                Validation (vm name, username), random username,
│                          SHA-512 password hashing, root check, shared prompts.
└── commands/
    ├── init.rs            First-run: interactive wizard or `--yes` silent mode.
    ├── pull.rs            Atomic image download (.partial → rename).
    ├── create.rs          Reads distro, generates seed, full-copies the base,
    │                      runs virt-install --import. Auto-pulls the base if
    │                      missing (docker-style).
    ├── delete.rs          Stops, undefines (handles UEFI nvram), removes files,
    │                      verifies, reports honestly if libvirt still has it.
    ├── lifecycle.rs       start/stop/restart/kill — virsh passthroughs.
    ├── info.rs            ls / inspect / ip / ssh-cmd.
    ├── images.rs          distros / images listings.
    ├── resources.rs       set-cpu / set-ram / resize-disk.
    ├── vnc.rs             qvm vnc: prints connect info (canonical host:display
    │                      AND host::port forms — viewers disagree on which
    │                      they accept). --browser spawns websockify + noVNC and
    │                      prints a URL + ASCII QR code.
    ├── doctor.rs          External dep check + install (apt/dnf/apk/pacman).
    └── completions.rs     Shell completion script generator.

src/tui/
    ├── mod.rs             Terminal init/teardown, panic-hook, main event loop.
    │                      The ONLY file that touches raw mode. Mouse capture
    │                      is intentionally never enabled (see § 9.5).
    ├── app.rs             State machine: Mode enum, FocusPane, pending
    │                      optimistic state, refresh logic with IP cache.
    ├── refresh.rs         Background refresh worker (mpsc channel). Keeps
    │                      the UI non-blocking while libvirt is slow.
    ├── theme.rs           Single source of visual truth. Catppuccin Mocha
    │                      (default) + Latte; helpers for state badges, hints.
    ├── ui.rs              Pure render functions. Consumes &Theme + &mut App.
    ├── events.rs          Keymap per Mode. Keyboard only.
    ├── onboard.rs         First-run TUI wizard (7 steps).
    └── forms.rs           Minimal text-input helper (avoids tui-input dep).

tests/
    ├── util_tests.rs        VM name validation, username, password hash format.
    ├── config_tests.rs      TOML parsing, layering, default values, paths.
    ├── cloudinit_tests.rs   Generated user-data has correct hostname/keys/script.
    ├── libvirt_tests.rs     parse_vnc_display — pure parser, no virsh needed.
    └── tui_app_tests.rs     App::new starts in EmptyState; toast variants.

integration/                INTEGRATION (bash smoke tests, run manually)
    ├── 05-self-contained.sh Most important. Verifies VM disks have NO backing
    │                        file (enforces invariant 3.1 against regression).
    └── …others              create/start/stop/delete end-to-end on a real host.
```

`tests/` (cargo) vs `integration/` (bash) is deliberate.
See [§ 15](#15-test-strategy).

---

## 6. Lifecycle of a VM

The full path from `qvm run web01 debian:13` to a running VM, in
diagram form first then prose:

```
   $ qvm run web01 debian:13
              │
              ▼
   ┌────────────────────────────────────────┐
   │ clap parses Cmd::Create → dispatch     │
   └──────────────────┬─────────────────────┘
                      │
                      ▼
   ┌────────────────────────────────────────┐                error
   │ libvirt::require_absent('web01')       │ ── exists ──────► exit 1
   │ cmd::require('virt-install', ...)      │ ── tool missing ► exit 1
   └──────────────────┬─────────────────────┘
                      │ ok
                      ▼
   ┌────────────────────────────────────────┐
   │ resolve params (defaults from cfg)     │
   │   cpus = 2 · ram = 4G · disk = 50G     │
   │   user = vmXXXXXX (random)             │
   └──────────────────┬─────────────────────┘
                      │
                      ▼
   ┌────────────────────────────────────────┐                 ╭──────────────────╮
   │ cfg.image_path(distro) exists?         │ ── missing ───► │ pull::pull_one   │
   │   /var/lib/qvm/images/debian-13.qcow2  │                 │ wget atomic      │
   └──────────────────┬─────────────────────┘ ◄────── ok ──── │ (.partial → mv)  │
                      │ yes                                   ╰──────────────────╯
                      ▼
   ┌────────────────────────────────────────┐
   │ Seed::build  (cloud-init NoCloud)      │
   │   · write_files()  → user-data,        │
   │     meta-data, .vmuser sidecar         │
   │   · build_iso()  → genisoimage         │
   │     → /var/lib/qvm/cloudinit/web01.iso │
   └──────────────────┬─────────────────────┘
                      │
                      ▼
   ┌────────────────────────────────────────┐
   │ qemu-img convert -O qcow2              │   ← FULL COPY, no -b flag
   │   <base>  →  /var/lib/qvm/vms/web01    │     (see § 2)
   │ qemu-img resize -q  web01.qcow2 50G    │
   └──────────────────┬─────────────────────┘
                      │
                      ▼
   ┌────────────────────────────────────────┐
   │ virt-install --import                  │
   │   --memory 4096 --vcpus 2              │
   │   --cpu host-passthrough (or host-model│
   │       -vmx -svm if nested=false)       │
   │   --disk path=web01.qcow2,bus=virtio   │
   │   --disk path=web01.iso,device=cdrom   │
   │   --osinfo name=debian12,require=off   │
   │   --graphics vnc,listen=<bind>         │
   │   --network bridge=<bridge>            │
   │   --channel ... qemu-guest-agent       │
   │   --memballoon model=virtio            │
   │   --noautoconsole                      │
   └──────────────────┬─────────────────────┘
                      │
                      ▼
   ┌────────────────────────────────────────┐
   │ if cfg.defaults.autostart:             │
   │   virsh autostart web01                │
   └──────────────────┬─────────────────────┘
                      │
                      ▼
   ┌────────────────────────────────────────┐
   │ print summary · qvm exits              │
   │ libvirt now owns the VM                │
   └──────────────────┬─────────────────────┘
                      │
                      ▼
        inside the guest, cloud-init's NoCloud datasource
        scans for the CD-ROM, reads user-data, runs the
        generic first-boot script (see § 7).
```

Walking through the same flow:

1. **clap parses** `Cmd::Create { name: "web01", distro: Some("debian:13"), … }`
   in `src/main.rs`. Dispatch routes to `commands::create::run`.

2. **Precondition checks** in `commands::create::run`:
   - `libvirt::require_absent(name)` — domain must not already exist.
   - `cmd::require("virt-install")`, `cmd::require("qemu-img")`,
     `cmd::require("genisoimage")` — host tools must be present.

3. **Resolve params** — defaults from `cfg.defaults` filled in where
   the user didn't specify (`cpus=2`, `memory_gb=4`, `disk_gb=50`).
   If no `--user`, a random `vm<6 alnums>` is generated via
   `util::random_username` and printed so the user knows the login.

4. **Base image** — `cfg.distro("debian:13")` looks up the distro
   record from the registry. `cfg.image_path("debian:13")` resolves to
   `/var/lib/qvm/images/debian-13.qcow2` (or whatever `[paths] images`
   points at).

5. **Auto-pull** — if the base file doesn't exist, `create.rs` prints
   `Unable to find image 'debian:13' locally, pulling...` and calls
   `commands::pull::pull_one(&cfg, "debian:13")`. That:
   - `wget` downloads the URL from the distro record into
     `<image>.partial`.
   - On success, renames to `<image>` (atomic). On failure, the
     partial is unlinked.

6. **Cloud-init seed** — `cloudinit::Seed::build(&ci_dir, &iso_path)`:
   - Writes `user-data`, `meta-data`, and a `.vmuser` sidecar into
     `<cloudinit>/web01/`.
   - Runs `genisoimage -joliet -rock` to package them as
     `<cloudinit>/web01.iso`. (cloud-init's NoCloud datasource picks
     this up at first boot.)

7. **Self-contained disk** —
   ```
   qemu-img convert -p -O qcow2 <base>.qcow2 <vms>/web01.qcow2
   qemu-img resize -q <vms>/web01.qcow2 50G
   ```
   The `-p` shows progress; the user sees a percentage tick by. **No
   `-b` flag.** Resize grows the qcow2; the guest will see the extra
   space and (typically) `growpart` it on first boot.

8. **Define + start** —
   ```
   virt-install \
     --name web01 \
     --memory 4096 --vcpus 2 --cpu host-passthrough \
     --disk path=<vms>/web01.qcow2,format=qcow2,bus=virtio \
     --disk path=<cloudinit>/web01.iso,device=cdrom \
     --osinfo name=debian12,require=off \
     --graphics vnc,listen=127.0.0.1 \
     --network bridge=br0,model=virtio \
     --channel unix,target_type=virtio,name=org.qemu.guest_agent.0 \
     --import --noautoconsole
   ```
   Notes:
   - `host-passthrough` for the CPU model — maximum performance,
     nested virt works out of the box.
   - `--osinfo name=X,require=off` instead of `--os-variant X` —
     older osinfo-db packages don't recognise every name; with
     `require=off` we degrade gracefully.
   - The cloud-init ISO is attached as a virtual CD-ROM. cloud-init's
     NoCloud datasource scans for it and consumes it on first boot.
   - VNC binds to whatever `[vnc] bind` says — default `127.0.0.1`,
     accessible only via SSH tunnel.

9. **Autostart** (default on) — `virsh autostart web01` runs so the VM
   restarts after a host reboot. Disabled by `--no-autostart`.

10. **First boot inside the guest** — cloud-init runs the script we
    wrote in step 6. See [§ 7](#7-cloud-init-pipeline).

11. **qvm exits**. The VM keeps running; libvirt is the owner now.

For the running VM:

- `qvm ls` → `virsh list --all`
- `qvm ip web01` → `virsh domifaddr web01 --source agent | lease | arp`
- `qvm ssh-cmd web01` → reads `<cloudinit>/web01/.vmuser` and combines
  with `qvm ip` output to print `ssh vm7f3a9c@10.0.0.42`.
- `qvm vnc web01` → reads `cfg.vnc.bind` + `virsh vncdisplay web01`,
  prints the canonical `host:display` form **and** the explicit
  `host::port` form (viewers disagree on which they accept).

Stopping:

- `qvm stop web01` → `virsh shutdown web01` (sends ACPI shutdown).
- `qvm kill web01` → `virsh destroy web01` (force-off).
- `qvm restart web01` → `virsh reboot web01`.

Deletion (`qvm rm web01`):

1. Confirm prompt (unless `-f`).
2. If running, `virsh destroy` (best-effort).
3. `virsh undefine web01 --nvram` — must come *before* removing disk
   files. If undefine fails (libvirt thinks the VM is still running, or
   NVRAM is locked), we error out **without** removing files. This
   prevents the "wedged VM" state where libvirt has a defined domain
   pointing at non-existent files.
4. Remove the qcow2, ISO, and cloud-init dir.
5. `virsh dominfo web01` to confirm it's gone; warn if not.

The order matters. The bash predecessor used to remove files first, then
undefine — which silently failed on UEFI VMs because of the NVRAM file,
leaving the user with a "ghost VM" in `virsh list --all` that pointed
at deleted disks.

---

## 7. Cloud-init pipeline

The seed is a NoCloud datasource: a small ISO with `user-data` and
`meta-data` in cidata volume label.

### 7.1 The user-data we emit

Generated by `cloudinit::Seed::user_data()`:

```yaml
#cloud-config
hostname: web01
manage_etc_hosts: true
ssh_pwauth: true
packages: [qemu-guest-agent]
users:
  - name: vm7f3a9c
    groups: sudo
    shell: /bin/bash
    sudo: ALL=(ALL) NOPASSWD:ALL
    lock_passwd: false
    passwd: "$6$rounds=5000$..."
    ssh-authorized-keys:
      - ssh-ed25519 AAAA... you@host
  - name: root
    ssh-authorized-keys:
      - ssh-ed25519 AAAA... you@host
write_files:
  - path: /opt/qvm-firstboot.sh
    permissions: '0755'
    content: |
      <firstboot script — see § 7.2>
runcmd:
  - /opt/qvm-firstboot.sh
```

- Random login user + root both get the same SSH keys from
  `cfg.ssh_keys`.
- Password is SHA-512 crypt'd by `util::hash_password`.
- `manage_etc_hosts` makes the guest write its own hostname to
  `/etc/hosts` so `sudo` doesn't print warnings.

### 7.2 The first-boot script (distro-agnostic)

`cloudinit::firstboot_script()` returns a shell script that runs
exactly once. The whole point of this file is to avoid per-distro
branches at the Rust level. Concretely:

```sh
#!/bin/sh
# Generated by qvm - runs once on first boot.

# --- GRUB timeout (best-effort across distros) ---
T="0"
if [ -n "$T" ]; then
    if [ -f /etc/default/grub ] && grep -q '^GRUB_TIMEOUT=' /etc/default/grub; then
        sed -i "s/^GRUB_TIMEOUT=.*/GRUB_TIMEOUT=$T/" /etc/default/grub
    else
        echo "GRUB_TIMEOUT=$T" >> /etc/default/grub
    fi
    if command -v update-grub      >/dev/null 2>&1; then update-grub || true
    elif command -v grub2-mkconfig >/dev/null 2>&1; then
        [ -d /boot/grub2 ] && grub2-mkconfig -o /boot/grub2/grub.cfg \
            || grub2-mkconfig -o /boot/grub/grub.cfg || true
    elif command -v grub-mkconfig  >/dev/null 2>&1; then
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
```

It uses `command -v` and `[ -f ]` to detect what's available. There is
no `case "$ID" in ubuntu)...` anywhere — distros come and go, but tool
names like `update-grub`, `grub2-mkconfig`, `systemctl`, `rc-update`
are stable across many distros.

### 7.3 Why split `Seed::build` into `write_files` + `build_iso`

`Seed::build` originally did everything: `require("genisoimage")`,
write files, run `genisoimage`. That made the function impossible to
unit-test without `genisoimage` installed (and CI hosts often don't
have it).

Now `Seed::write_files` writes user-data/meta-data/.vmuser into a temp
dir — pure I/O, no external binary required. The unit tests use this
half. `Seed::build_iso` runs `genisoimage` on already-written files; if
that fails, the files are still on disk for the operator to inspect.

`Seed::build` is just the convenience wrapper.

---

## 8. Distro registry (data-driven)

`src/config.rs::builtin_distros()` returns a `BTreeMap<String, Distro>`:

```rust
pub struct Distro {
    pub image:  String,  // filename under [paths].images
    pub osinfo: String,  // libosinfo id — passed with require=off
    pub shell:  String,  // login shell (Alpine is /bin/sh; others /bin/bash)
    pub uefi:   bool,    // true only for Alpine's cloud image
    pub url:    String,  // stable URL to download from
}
```

To add a sixth distro:

```yaml
# /etc/qvm/config.yml
distros:
  "ubuntu:22.04":
    image:  ubuntu-22.04.qcow2
    osinfo: ubuntu22.04
    shell:  /bin/bash
    uefi:   false
    url:    https://cloud-images.ubuntu.com/releases/jammy/release/ubuntu-22.04-server-cloudimg-amd64.img
```

That's it. No Rust changes, no recompile, no testing.

The `uefi: true` flag only matters for Alpine, because Alpine's
BIOS-bootable nocloud cloud image has a known issue where it hangs at
"Loading initramfs-virt" inside libvirt due to syslinux/SeaBIOS quirks.
The UEFI variant boots correctly. Ubuntu/Debian/Fedora/Rocky all boot
fine on BIOS, so we don't pay the UEFI NVRAM management overhead for
them.

`create.rs` reacts to `uefi: true` by appending
`--machine q35 --boot uefi,loader.secure=no` to virt-install. That's
the only branch on the flag.

---

## 9. The TUI internals

The TUI is in `src/tui/`. It is the primary interactive admin surface.
It is a thin presenter — every action it takes delegates to one of the
`commands::*` functions used by the CLI.

### 9.1 Layout

The TUI is built from four regions, laid out top-to-bottom:

- **Header** (1 line): brand · hostname · VM summary · context hint.
- **Body** (fills the middle): split horizontally on ≥80-col terminals
  into a 28-col sidebar (VM list + filter + "+ create new VM") and a
  content pane that renders different things per `Mode`. Narrower
  terminals collapse to a single sidebar-only view.
- **Toast** (1 line, only when active): the latest success/error
  message, auto-dismissed after 5 s.
- **Action bar** (bordered panel, 1-3 rows): labelled `[k] Label`
  hints. `fit_action_rows` greedily packs them onto as few rows as
  fit the terminal width. Disabled hints (e.g. Stop when nothing is
  running) render faint. Triggered by the bracketed key only —
  keyboard-only by design (see § 9.5).

`Tab` cycles `FocusPane::{Sidebar, Detail}`; the active pane gets a
mauve border instead of the dim default. Render functions are pure
(no I/O); all colours come from `src/tui/theme.rs`.

### 9.2 State machine

`Mode` lives in `src/tui/app.rs`. Transitions happen in `App::apply`
(triggered by keypresses) and in `App::refresh` (transitions to/from
`EmptyState` when the row count crosses zero).

The state machine is small (six modes) and best shown as a table:

```
  Mode              Entered by    How you leave it
  ────────────────  ────────────  ──────────────────────────────────────────
  Detail            (default)     q · Esc → quit
  EmptyState        no VMs found  → Detail automatically when rows appear
  CreateForm        c             Enter → commands::create::run, back to Detail
                                  Esc   → back to Detail
  ConfirmDelete     d             y → commands::delete::run, back to Detail
                                  n · Esc → back to Detail
  Filter            /             Enter (commit) · Esc (cancel) → Detail
  Help              ?             any key → Detail

  Two actions don't change Mode — they suspend ratatui and exec a child
  process, then restore the TUI on return:

  Key   Action                              Returns when
  ────  ──────────────────────────────────  ──────────────────────
  e     virsh console <name>                Ctrl-] (virsh escape)
  b     websockify + noVNC + URL + QR       Ctrl-C
```

`Tab` cycles `FocusPane::{Sidebar, Detail}` independently of `Mode`.

`Mode` is in `src/tui/app.rs`. Transitions happen in `App::apply`. The
crossterm key map in `events.rs` routes keys differently per mode —
this is what lets `/` open filter from the sidebar but be a regular
text character inside the filter input.

### 9.3 The refresh loop and the IP cache

The main loop in `tui/mod.rs` calls `event::poll(100ms)` and on each
iteration:

1. Renders the current frame.
2. Increments `app.tick` (used by the spinner glyph).
3. Polls for keyboard events; dispatches if any.
4. If `app.tick_due()` (≥ 2 s since last refresh), calls `app.refresh`.

`App::refresh` is synchronous (no tokio, no threads). It does:

- `libvirt::domains()` — fast: one `virsh list --all --name` + one
  `virsh domstate` per VM.
- For each running VM, **looks up its IP from `prev_ips` first**; only
  shells out to `libvirt::ipv4()` if we don't already have one cached.
  This is critical because `ipv4` runs up to three virsh calls (agent,
  lease, ARP) and is the slowest part of refresh. With the cache, a
  steady-state refresh is ~5–10 ms total even with 10 VMs.

The early implementation re-shelled-out for every IP on every refresh,
causing visible UI freezes every 2 s. The IP cache eliminated those
freezes. See the `refresh` doc comment in `app.rs` for the why.

### 9.4 Optimistic state ("starting…")

When the user presses `s` on a stopped VM, libvirt's `start` call takes
~3–5 s. Without optimistic state, the row's `state` column would stay
"shut off" until the next 2-s refresh tick — making the click feel
unresponsive.

`App::pending: Option<(String, &'static str)>` records the in-flight
transition. The UI's `displayed_state(row)` returns the pending label
("starting…") for the affected VM, real state for everyone else. The
next refresh clears `pending` and the row's displayed state becomes the
real state.

Action-bar enables also use `displayed_state` so the buttons don't
flicker direction (e.g., Start enabled → disabled → enabled).

### 9.5 Why keyboard-only (no mouse capture)

The TUI used to capture mouse events for sidebar clicks and action-bar
buttons. That was removed in commit `fix(tui): mouse capture leak —
disable on suspend + panic`. The root cause:

When ratatui enables mouse capture, the terminal switches to a tracking
mode that emits escape sequences (`^[[<35;X;YM` for SGR mode 1003)
**every time the mouse moves**. Those sequences are only meaningful while
the program is running. If the program exits without explicitly disabling
capture (panic, SIGINT during a `suspend()`-ed child, kernel kill), the
terminal stays in tracking mode — and the very next shell prompt fills
with garbage on every cursor twitch. Recovery requires `reset`.

We had three exit paths that could leak:
1. Panic in render or refresh worker.
2. SIGINT during `suspend()` (when raw mode is off and the child has the
   terminal — Ctrl-C kills qvm directly).
3. Kill -9 / OOM kill / SSH disconnect mid-execution.

We could have plugged #1 and #2, but #3 is unfixable: any abrupt death
with mouse capture on corrupts the parent shell. Keyboard navigation
already covers everything mouse did (sidebar via ↑/↓, action bar via
the bracketed key, scrolling via PgUp/PgDn). The trade is fine.

So `tui::run` never emits `EnableMouseCapture`, `events.rs` doesn't
handle mouse actions, and `app.rs` has no hit-test tables. The shell
prompt stays clean even on the worst exit path.

### 9.6 Theme

`src/tui/theme.rs` is the single source of truth for colours. Every
render fn takes `&Theme`. No `Style::default().fg(Color::...)` calls
outside `theme.rs`.

Default palette is Catppuccin Mocha. RGB values are pinned via
`Color::Rgb(r,g,b)` — every modern terminal that supports truecolor
(iTerm, kitty, alacritty, GNOME Terminal, Windows Terminal, WezTerm)
renders identically. Terminals without truecolor degrade to the closest
ANSI 256-color match — readable but not as polished.

`theme::keyhint(key, label, enabled)` is the canonical action-bar
button renderer. The full action bar uses `fit_action_rows()` (in
`ui.rs`) to greedily pack buttons onto as few rows as fit the terminal
width — single row on a 200-col terminal, two rows on 120, three rows
on 80. The bar height adapts at draw time.

---

## 10. The VNC console paths

There are three ways to see a VM's screen, picked deliberately for
three different audiences. The table:

| Path                                | What                          | When to use                                                |
|-------------------------------------|-------------------------------|------------------------------------------------------------|
| `virsh console` (TUI `e`)           | Serial console (text-mode)    | Cloud images at login prompt; reading boot messages        |
| Native VNC viewer (TUI `v`)         | Print connect info as a toast | You already have RealVNC / TigerVNC / Screen Sharing       |
| `qvm vnc --browser` (TUI `b`)       | Spawn websockify + serve noVNC| You're on a phone, or you don't want to install a viewer  |

All three paths reach the same VM through different transports:

- **Serial console** (`e`) attaches a PTY to the guest's serial port
  via `virsh console`. qvm suspends ratatui, the user interacts, Ctrl-]
  detaches.
- **Native VNC** (`v`) reads `cfg.vnc.bind` + `virsh vncdisplay <name>`
  and prints `host:display`, `host::port`, and `vnc://host` strings —
  the user runs their own viewer.
- **Browser bridge** (`b`, or `qvm vnc --browser`) `cmd::exec`s
  `websockify` to bridge a WebSocket→TCP between `0.0.0.0:6080` and
  the VM's VNC port. websockify's `--web` flag serves the
  system-installed noVNC bundle so the user just opens an HTTP URL.
  An ASCII QR of the URL is printed alongside.

Read the table at the top of this section to decide which one fits
your situation.

### 10.1 The serial console path (`e`)

The trick that took two iterations to get right: `cmd::run_inherit`
nulls stdin (correct for batch programs like wget and qemu-img).
`virsh console` needs **a real stdin**, so we added `cmd::run_tty`
which inherits all three stdio. The TUI's `Action::Console` calls
`run_tty`. The error before this fix was: `Cannot run interactive
console without a controlling TTY`.

### 10.2 The native VNC info path (`v`)

`commands::vnc::run` prints:

```
VNC for 'web01':
  bind     127.0.0.1
  display  :0
  port     5900

From a VNC viewer:
  vncviewer 127.0.0.1:0          # canonical: host:display
  vncviewer 127.0.0.1::5900      # explicit port form (always works)

From macOS Screen Sharing:
  open vnc://127.0.0.1

Loopback bind — first tunnel via SSH:
  ssh -L 5900:127.0.0.1:5900 root@aether
  # then on your laptop:
  open vnc://127.0.0.1
```

Why two `vncviewer` forms? In RFB convention, `host:N` means **display
N** (port 5900+N). `host::P` means **port P** explicitly. The original
version of qvm printed `vncviewer host:5900`, which most clients read as
display 5900 → port 11800 → connection refused. We now print both
forms; if your viewer accepts the first, great; if not, the explicit
form always works.

`libvirt::vnc_endpoint` returns both `display` and `port` as a
`VncEndpoint` struct, parsed from `virsh vncdisplay`. `parse_vnc_display`
is exposed for unit tests.

### 10.3 The browser path (`b` in TUI, `qvm vnc --browser` from CLI)

```
$ qvm vnc --browser web01

Browser VNC for 'web01':
  Open in any browser on this LAN:
    http://10.1.1.10:6080/vnc_lite.html?host=10.1.1.10&port=6080&autoconnect=true&resize=scale&reconnect=true

    █████████████████████████████████████████████████████
    █████████████████████████████████████████████████████
    ████ ▄▄▄▄▄ █████ ▀█▀▀█▀▀ ████ ▀█▄▀▀▀▀█▀█ ▄▄▄▄▄ ████
    ████ █   █ ██▀▀█ █▀▄  ▀▀▀▄ █ ▀█▀  ▄▀▄▄ █ █   █ ████
    ████ █▄▄▄█ █ ██ █  █▀▀▀ ▀█ ▀█ ▀█▄ █▄ ▀▀ █ █▄▄▄█ ████
    ████▄▄▄▄▄▄▄█ ▀▄█ █ ▀ █▄█ ▀▄▀ █▄█ █ ▀▄▀ █▄▄▄▄▄▄▄████
    …(more QR rows)…
    ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀

Press Ctrl-C to stop the bridge.
```

The QR is the URL above, encoded as Unicode half-block characters
(`█▀▄`). Scan it from a phone camera → opens the noVNC page in mobile
Safari/Chrome → console works on the phone. Crate used:
`qrcode v0.14` (pure Rust, no_std, ~50 KB).

After printing, qvm `cmd::exec`s `websockify` — the shell-level `exec`
that replaces the qvm process with websockify. When the user Ctrl-C's,
websockify receives SIGINT and exits cleanly; the user is back at the
shell.

Bridge dial target: qvm reads `cfg.vnc.bind` (e.g. `127.0.0.1` or
`10.1.1.10`) + the VNC port from `vnc_endpoint`. websockify connects to
that. websockify itself listens on `0.0.0.0:6080` so anyone on the LAN
can reach it. The audience is trusted-LAN homelab; we don't auth the
bridge.

From inside the TUI, pressing `b` suspends ratatui and calls into the
same `commands::vnc::run(.., browser=true)` function. The QR is printed
to the now-restored cooked terminal, websockify takes over, Ctrl-C
returns to the TUI. Zero code duplication between CLI and TUI for the
browser bridge — it's literally the same function call.

---

## 11. Error model

`src/error.rs` defines exactly four variants, deliberately small:

```rust
pub enum Error {
    User(String),                  // anything we want to show the user verbatim
    Command { cmd, status, stderr },  // external process failed; stderr is useful
    Io(std::io::Error),            // thin wrapper
    Toml(toml::de::Error),         // thin wrapper
}
```

No `anyhow`, no `color-eyre`, no stack traces at the boundary. When
something goes wrong, the user sees one of:

```
Error: invalid VM name 'My VM'. Use letters, digits, . - _ and start alnum.
Error: command `virsh` failed (exit 1): error: failed to get domain 'web02'
```

…both of which contain everything they need. The User variant is for
input validation, "not found", and other foreseeable problems. The
Command variant is for "the external tool said no, here's why" — its
stderr is the actual content.

`Result<T> = std::result::Result<T, Error>` is re-exported from
`error.rs`. Every public function in `commands::*` returns this type.

---

## 12. Extension points

### Adding a new distro

Edit `/etc/qvm/config.yml`:

```yaml
distros:
  "alpine:3.21":
    image:  alpine-3.21.qcow2
    osinfo: alpinelinux3.21
    shell:  /bin/sh
    uefi:   true
    url:    https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/cloud/nocloud_alpine-3.21.0-x86_64-uefi-cloudinit-r0.qcow2
```

Run `qvm pull alpine:3.21`. Done. No code changes.

### Adding a new top-level command

1. Add a variant to `Cmd` in `src/main.rs`.
2. Add a dispatch arm.
3. Create `src/commands/<your_cmd>.rs` with a public `run(cfg, args)`
   function returning `Result<()>`.
4. Add a `pub mod <your_cmd>;` line to `src/commands/mod.rs`.

The command function does its job using `cmd::*`, `libvirt::*`, and
`config::*`. No other coupling.

### Theming the TUI

Edit `src/tui/theme.rs::Theme::default()` and change the RGB values.
The whole TUI re-themes from one place.

Eventually we might let users supply a `[tui.theme] palette = "tokyo-night"`
in config, but until someone asks, Catppuccin Mocha is the default.

### Hooking new render in the TUI

Render functions in `src/tui/ui.rs` are pure: they take `&mut App` (or
`&App` if they don't push hit-test rects) and a `Frame + Rect`, and
return nothing. The state machine in `app.rs` decides which renderer
runs (via `Mode`). To add a new screen — say, "Hardware" tab — you'd:

1. Add a `Mode::Hardware` variant.
2. Add a key binding in `events.rs` (e.g. `h` → `OpenHardware`).
3. Add a `draw_hardware` function in `ui.rs`.
4. Route `Mode::Hardware` to `draw_hardware` in `draw_content`.

---

## 13. What qvm deliberately doesn't have

A list of things people often expect, that aren't there on purpose:

- **No `anyhow` / `color-eyre`.** The two-variant Error works. We never
  need a stack trace at the boundary.
- **No `tokio` or async.** Nothing in this tool is I/O-bound enough to
  benefit, and async runtimes balloon binary size dramatically.
- **No daemon.** libvirtd is the daemon. qvm processes are
  short-lived: start, do a thing, exit.
- **No background workers in the TUI.** Refresh is synchronous; we
  cache IPs to make it fast. If steady-state refresh ever becomes
  visibly slow, the right answer is a background thread + mpsc channel
  to the main loop, not async.
- **No plugin system.** A single binary, no script dirs, no addons.
- **No persistent web admin UI.** We built it (commits `694e16d` to
  `acf3603`), tested it on `aether`, and removed it. The TUI is the
  better home for management. The browser remains useful only for the
  VNC console.
- **No backing-file qcow2 creation.** See [§ 2](#2-the-disaster-that-birthed-this-tool).
- **No multi-host or clustering.** qvm manages one host. To manage
  more, run qvm on each.
- **No live migration.** Out of scope.
- **No image building / Packer.** Out of scope.
- **No auth or TLS.** Trusted-LAN homelab tool.
- **No internationalisation.** English only for now.

---

## 14. Build, release, install

### Local development build

```
cargo build --release
./target/release/qvm --help
```

### Static musl binary (what CI produces, what users install)

```
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools          # on Debian/Ubuntu build hosts
cargo build --release --target x86_64-unknown-linux-musl
file target/x86_64-unknown-linux-musl/release/qvm
# → ELF 64-bit LSB pie executable, x86-64, static-pie linked, stripped
```

The result is ~2 MB. It runs on any Linux x86_64 host with the system
tools (virsh, virt-install, qemu-img, qemu-system-x86_64, genisoimage,
wget) installed. `qvm doctor --install` installs those tools via the
host's package manager.

### Continuous integration

`.github/workflows/build.yml` runs the test suite, then produces a
`qvm-linux-amd64-static` artifact on every push and PR. The artifact is
the static-musl binary. CI deliberately does not create GitHub
Releases; that's a manual decision per release.

### Distribution to a host

The one-liner installer (`install.sh`) downloads the latest CI
artifact via nightly.link, installs it to `/usr/local/bin/qvm`, runs
`qvm doctor --install --yes` to install system deps, and installs
shell completions. It does **not** call `qvm init` — configuration is
something the user runs explicitly. See `install.sh` for the full
flow.

---

## 15. Test strategy

Two separate test suites, deliberately split:

### 15.1 `tests/` (workspace root) — unit tests, run by `cargo test`

- Pure Rust logic only: validation, config parsing/layering, cloud-init
  YAML structure, sidecar IO, distro registry invariants, VNC display
  parser.
- Currently 44 tests across 5 files.
- All run in milliseconds.
- No libvirt, no genisoimage, no network required.
- Runs in CI on every push.

### 15.2 `integration/` — bash smoke tests

- Bash scripts that exercise the *installed* `qvm` binary against a
  real libvirt host with KVM. (Formerly `test/` — renamed to avoid
  confusion with Cargo's `tests/`.)
- Actually create VMs, wait for them to boot, ssh in, delete them.
- Each script self-cleans (via `trap EXIT`).
- The most important one is `integration/05-self-contained.sh`: it
  confirms that VM disks have **no backing file**. If this regresses,
  we're back to the Dev/Hermes disaster mode of [§ 2](#2-the-disaster-that-birthed-this-tool).
- **Not** run in CI. Run them manually after deploying to a new host,
  upgrading libvirt/qemu/kernel, or touching create/delete/cloudinit.

If you're tempted to merge the two suites — don't. `cargo test` should
stay fast and side-effect-free; integration tests should be honest
about what they need (a real KVM host, network, time).

---

## 16. Open work

Concrete things that aren't done, in roughly the order they'd help:

- **`qvm export` / `qvm import`** — qemu-nbd-based data extraction.
  The manual recovery procedure (mount via qemu-nbd, rsync
  `/root /home /opt /srv /var/lib/docker/volumes /etc-selected`) is
  well-tested but not yet a command.
- **`qvm flatten`** — detach an existing overlay-based VM from its
  backing file. The migration tool for VMs that still exist from the
  old bash predecessor. Not strictly needed if everything was created
  by qvm, but useful for adopters with legacy VMs.
- **`qvm console`** — thin CLI wrapper around `virsh console <name>`.
  The TUI `e` action already does this; a CLI subcommand would let
  scripts/cron do the same without entering the TUI.
- **Background-thread refresh** in the TUI — when refresh ever becomes
  visibly slow, this is the fix. mpsc channel from worker to main loop;
  the spinner becomes a real animation.
- **Tabbed detail pane** — Summary / Console / Hardware / Cloud-init.
  Proxmox-style. Currently the Shift+R toggle covers ~80% of the value.
- **Light-theme variant + theme picker in config.** Catppuccin Latte
  defaults plus a `[tui.theme] palette = "..."` knob.
- **Snapshots** — thin passthrough over `virsh snapshot-*`. We
  intentionally don't reinvent these.

The trio of `export` + `import` + `flatten` would close the only
remaining manual-recovery procedure. They're high-value for adopters
migrating from other tools.
