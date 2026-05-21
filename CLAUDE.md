# CLAUDE.md

Companion document for AI assistants and contributors. The README is for users
of the binary; this file explains **why** the code looks the way it does, what
problem it solves, and the design constraints behind every non-obvious choice.

If you change a non-trivial design decision in the code, update the matching
section here.

---

## 1. The problem that made this exist

`qvm` was born out of a production disaster.

The previous tool was a bash script (`vm`) that created each VM as a thin
qcow2 **overlay** on top of a shared base image (`-b /SSD/Images/ubuntu.img`).
Overlays only store changed blocks; unchanged blocks (kernel, /lib, /usr) are
read at boot time from the backing file.

Then, months later, the user re-ran `vm pull ubuntu24`, which downloaded a
newer Ubuntu daily build over the existing base file. Every VM that depended
on the old base now read **a different Ubuntu** for any block it hadn't
written. Symptoms:

- GRUB rescue prompt on boot ("invalid arch-independent ELF magic")
- After we forced a direct kernel boot, `/sbin/init` failed to load shared
  libraries (the libs on disk were partial garbage from the new base)
- Config files like `/etc/mdadm/mdadm.conf` contained literal binary noise
- libguestfs auto-inspection refused to mount because it couldn't parse
  augeas configs

Two production VMs were nearly lost. Recovery required mounting via
`qemu-nbd`, extracting user data, and rebuilding from scratch.

**The root cause was architectural, not a code bug.** Any tool that creates
overlay-on-shared-base VMs by default has this disaster mode, and there is no
amount of "be careful when you `pull`" warnings that protects against it.

Hence rule #1 of qvm: **every VM disk is a full, self-contained copy. There
are no backing files. Ever.**

This costs ~10–30 seconds and ~500MB–1GB of disk per VM at create time.
That trade is worth making once, forever, instead of explaining "don't run
pull while VMs exist" to someone (including future you) at 2 AM.

---

## 2. Scope

### In scope

- Create / start / stop / restart / kill / delete VMs (single-name **and**
  multi-name batch: `qvm stop a b c` or `qvm stop --all`)
- List, inspect, get IP, get an SSH command, and **`qvm ssh <vm>`** that
  exec's ssh directly instead of just printing the command
- `--json` output on `qvm ls`, `qvm distros`, `qvm images` for automation
- Pull and list distro base images (5 baked in, more via config) — each
  with **both amd64 and arm64 variants**
- VNC connection info (replaces the need for Cockpit for graphical access)
- Resource changes (CPU, RAM, disk grow)
- **`qvm console <name>`** — drops into `virsh console` with proper TTY
  inheritance (Ctrl-] to detach)
- **`qvm snap {create,list,revert,rm,rotate}`** — passthrough over
  `virsh snapshot-*` with `--quiesce` / `--running` / `--keep N`
- **`qvm export`/`qvm import`** — package a VM into a `.qvm.tar` tarball
  (qcow2 + cloud-init seed + `.vmuser` sidecar + `qvm-meta.toml` with
  sha256). Live mode uses `snapshot-create-as --disk-only --quiesce` +
  `blockcommit --active --pivot` (crash-consistent, no downtime).
  Import refuses cross-arch via a `uname -m` check.
- **`qvm flatten <name>`** — `qemu-img convert` in place. Migration aid
  for VMs created by the bash predecessor.
- **`qvm cleanup`** — finds qcow2 / seed / sidecar files with no matching
  libvirt domain and removes them after confirmation. The TUI header
  surfaces a non-blocking "N orphans" hint on startup.
- One-time setup: `qvm init` launches the TUI onboarding wizard. `--yes`
  writes default config silently for automation. `--force` overwrites.
- **Interactive TUI** (bare `qvm` with no subcommand) — Proxmox-style
  split-pane with sidebar, contextual content, status bar. Background
  refresh worker (mpsc channel) keeps the UI responsive while libvirt
  is queried. Themes: Catppuccin Mocha (dark, default) + Latte (light).
- **`qvm vnc --browser`** — spawns a `websockify` + noVNC bridge for the
  one thing browsers genuinely do better than terminals: render the VM's
  framebuffer.
- **amd64 + arm64**: single static musl binary per arch. CI matrix builds
  both. `install.sh` switches on `uname -m`.

### Explicitly out of scope

These are excluded by design, not by accident. Do **not** add them without
re-reading section 1 and considering whether the tool is sprawling.

- Multi-host / clustering / federation
- Live migration (snapshot-based **export/import** is in scope; moving a
  running domain between hosts is not)
- Storage pools / LVM / Ceph / anything other than qcow2 files in a
  configurable directory
- **Web admin UI for VM management.** We tried it (commits 694e16d…acf3603)
  and the TUI was the better home. The browser is only used for the VM
  console via `qvm vnc --browser` — see decision-log section 5.
- Persistent / daemonised processes of any kind. qvm exits when its
  invoking operation is done.
- User management or multi-tenant access (single root-owned tool)
- Image building or Packer-style workflows
- Container support (this is a VM tool)
- Automatic mirror selection / geo-pinning
- **Cross-arch emulation** (running x86 guests on ARM hosts via qemu TCG).
  The import path explicitly refuses on arch mismatch and tells the user
  to reinstall on the target arch.
- OVA / VMDK formats. `.qvm.tar` stays transparent and `tar tf`-inspectable.

If you want one of these, fork it. A small tool that does its job is more
valuable than a Swiss-Army knife that no one understands.

---

## 3. Hard rules the code obeys

These are non-negotiable and reflected in tests:

1. **Every VM disk is a full copy.** No `-b` / no backing file in any
   `qemu-img create`. See `commands/create.rs`: `qemu-img convert` followed
   by `qemu-img resize`. Verified by inspection (the only `qemu-img` call
   that produces a VM disk is `convert`).
2. **libvirt is the source of truth.** We do not maintain a parallel state
   file of "which VMs exist". `qvm ls` is `virsh list --all` plus light
   formatting. We keep exactly one tiny sidecar per VM: `<cloudinit>/<name>/.vmuser`
   — the login user, so `qvm ssh-cmd` can print the right username.
3. **No per-distro code branches.** Distro differences live in `config.rs`
   (data: image filename, osinfo id, shell, UEFI flag, URL). Behavioral
   differences (systemd vs OpenRC, update-grub vs grub2-mkconfig) are
   resolved **inside the guest** by a generic first-boot script that
   feature-detects. Adding a 6th distro is a config edit, not a code change.
4. **Shell out, don't link.** No libvirt-rs FFI, no qemu bindings. Every
   external interaction goes through `cmd::run` / `cmd::run_inherit`. This
   makes debugging trivial (`strace`, copy/paste failing command into a
   shell) and means qvm works on any host where the tools work, without
   library version dancing.
5. **Stable URLs, not dailies, for built-in distros.** Even with rule #1,
   we prefer URLs that point at point releases (e.g., Debian's
   `cloud/trixie/latest/` symlink to the latest stable, not
   `cloud/trixie/daily/latest/`).
6. **Config is single-file TOML.** One place to look. Defaults baked into
   the binary; any field overridable in `/etc/qvm/config.toml`.

---

## 4. Module map

```
src/
├── main.rs            CLI dispatch (clap). Trivial — all logic in commands/.
├── lib.rs             Library surface so the test suite can exercise modules.
├── error.rs           One Result type. Two variants: User (printed plain),
│                      Command (with stderr). No anyhow soup.
├── cmd.rs             The ONLY layer that touches external processes.
│                      run() / run_inherit() / run_tty() / exec().
├── arch.rs            Host arch detection (uname -m, normalized). Picks
│                      qemu_system_bin() + drives ARM-only virt-install flags.
├── libvirt.rs         Thin virsh wrapper. exists/is_running/start/stop/...
│                      ipv4() looks via agent, then DHCP lease, then ARP.
│                      undefine() handles UEFI NVRAM correctly.
├── config.rs          TOML schema, baked-in defaults, distro registry.
│                      Five built-in distros, each with per-arch variants
│                      (x86_64 + aarch64). User config layers over top.
├── cloudinit.rs       Seed generator. write_files() then build_iso().
│                      Split so tests can exercise file generation without
│                      needing genisoimage.
├── style.rs           CLI ANSI colors via owo-colors. Honors NO_COLOR + TTY.
├── util.rs            Validation (vm name, username), random username,
│                      SHA-512 password hashing.
└── commands/
    ├── init.rs        First-run wrapper. Delegates to tui::onboard for the
    │                  interactive flow, or writes defaults silently for --yes.
    ├── pull.rs        Atomic image download via embedded ureq HTTPS client
    │                  (no external wget). Exposes pull_one() for create.
    ├── create.rs      Reads distro+arch, generates seed, full-copies the base,
    │                  runs virt-install --import. Auto-pulls on missing image.
    │                  Adds --arch aarch64 --machine virt --boot uefi on ARM.
    ├── delete.rs      Stops, undefines (handles UEFI nvram), removes files,
    │                  verifies, reports honestly if libvirt still has it.
    ├── lifecycle.rs   Verb enum (Start/Stop/Restart/Kill) + batch() helper
    │                  so `qvm stop a b c` and `qvm stop --all` work.
    ├── console.rs     `qvm console <name>` — thin run_tty("virsh", ["console", n]).
    ├── cleanup.rs     `qvm cleanup` — finds qcow2 / seed-iso / seed-dir
    │                  files with no matching libvirt domain. Also exposed
    │                  to the TUI which renders an "N orphans" header hint.
    ├── flatten.rs     `qvm flatten <name>` — qemu-img convert in place.
    │                  Migration aid for VMs from the bash predecessor.
    ├── snap.rs        snapshot-create-as / list / revert / delete / rotate.
    │                  rotate --keep N parses snapshot-list by creation order.
    ├── export.rs      `qvm export` — packages a VM into .qvm.tar with
    │                  disk.qcow2, cloud-init.iso, .vmuser, domain.xml, and
    │                  qvm-meta.toml (arch, cpus, memory, sha256). Live mode
    │                  uses snapshot-create-as --disk-only --quiesce +
    │                  blockcommit --active --pivot.
    ├── import.rs      `qvm import` — extracts a tarball, verifies sha256,
    │                  refuses cross-arch, rebuilds the domain via
    │                  virt-install --import with --cpu host-model.
    ├── info.rs        ls (--json) / inspect / ip / ssh-cmd / ssh-exec.
    ├── images.rs      distros (--json) / images (--json) listings.
    ├── resources.rs   set-cpu / set-ram / resize-disk.
    ├── doctor.rs      Host dependency check. Arch-aware (picks qemu-system-X).
    │                  deps_for_host() builds the DEPS slice at runtime.
    ├── completions.rs Shell completion script (bash/zsh/fish/elvish/pwsh).
    └── vnc.rs         Prints the connect string (canonical `host:display`
                       AND explicit `host::port` forms — most viewers reject
                       the port-only form). --open launches a local viewer.
                       --browser starts a websockify + noVNC bridge with QR.

src/tui/
    ├── mod.rs         Terminal init/teardown, panic-hook, main event loop.
    │                  Spawns a background refresh worker via mpsc channel.
    │                  Only file that touches raw mode.
    ├── app.rs         State machine + apply() dispatch. Pure logic.
    │                  refresh() = sync (post-action); apply_async_refresh()
    │                  consumes results from the worker.
    ├── refresh.rs     Background worker thread. Sleeps 2s, sends Starting +
    │                  Result(rows, selected_dominfo) on the channel.
    ├── ui.rs          Pure render functions for sidebar/detail/header/bar.
    ├── events.rs      Crossterm key events → Action enum.
    ├── forms.rs       Minimal text-input helper (avoids tui-input dep).
    ├── theme.rs       Catppuccin Mocha (default) + Latte (light), selected
    │                  from [tui] theme = "mocha"|"latte" in config.
    └── onboard.rs     First-run TUI wizard. 7 steps: welcome, host-check,
                       network (bridge validation), ssh keys, paths
                       (writability check), first image (HEAD reachability
                       check), done. Reuses commands::init::render_config.

```

---

## 5. Decision log

### Why TOML for config and not YAML

YAML is more familiar to cloud-init users but has whitespace traps and
ambiguities (`no` parses as boolean false in YAML 1.1). TOML has one obvious
way to write things and Cargo/rustup already establish the file format in
the user's mental model. The cloud-init user-data we emit is YAML — we have
to write YAML — but the *config* the user edits is TOML.

### Why no `anyhow` / `eyre`

Two error variants cover everything we surface:

- `Error::User(String)`: anything we want to show the user as a plain
  message — invalid input, unknown distro, "not found", etc. Printed verbatim.
- `Error::Command { cmd, status, stderr }`: external-process failures.
  Stderr is the actual content the user needs.

Plus `Io` and `Toml` thin wrappers. We never need a stack trace at the
boundary; if a user sees `Error: command \`virsh\` failed (exit 1): ...` they
have everything they need.

### Why split `Seed::build` into `write_files` + `build_iso`

Originally `build` called `require("genisoimage")` first, then wrote files,
then ran the binary. This made the function impossible to test without
genisoimage installed (CI failed in this exact way during development).

Splitting also gives a natural recovery point: if the ISO build fails, the
files are still on disk for inspection.

### Why a random username by default instead of a fixed one

The bash predecessor hardcoded `k3s` as the default. This broke `vm ssh-cmd`
silently when someone created a VM with `-u other`: the script printed the
default user, not the actual one. With a random username (e.g. `vm7f3a9c`)
plus a sidecar (`.vmuser`) that the create step writes and the ssh-cmd step
reads back, the bug class disappears.

### Why `host-passthrough` for CPU

Maximum performance, nested virtualization works automatically if the host
supports it. The only thing it loses you is live migration across CPUs of
different models — and migration is out of scope (see section 2). For a
single-host homelab tool this is strictly better than `host-model`.

### Why the web management UI was removed (after shipping it)

I shipped `qvm web` as a full server-rendered management UI: VM list,
create form, lifecycle actions, inspect page, delete confirm, embedded
noVNC console (commits 694e16d / acf3603). It worked. Then we tested it
on `aether` and the user pushed back: the only thing a browser does
*better* than a terminal is render the VM's framebuffer. For every other
operation (list, create, start/stop, inspect, delete) the TUI is
faster, doesn't require a daemon, and doesn't carry HTML/CSS/JS in the
binary. The web UI was scope creep we walked back from.

What stayed: `qvm vnc --browser` — a foreground command that spawns
`websockify` + noVNC and prints a URL. Same UX shape (start on demand,
Ctrl-C to stop), but only for the one use case the browser actually wins.

What got deleted: `src/web/` (all of it), `src/commands/web.rs`, the
`Cmd::Web` clap variant, `tiny_http` and `signal-hook` dependencies,
the four `qvm web` polish commits' worth of CSS/JS/HTML.

The lesson is in `## 2 In scope` — when in doubt, the terminal is the
admin surface, the browser is for framebuffers.

### Why the TUI was redesigned (single-pane + modals → Proxmox split-pane)

The first TUI was a single-pane VM table with modal popups for every
action (create form, inspect, vnc info, delete confirm). It worked but
felt basic — "open modal, close modal" rhythm for everything, no
persistent context, no sense of "this is an admin tool". The user
asked for something closer to Proxmox VE.

Today's layout:

- **Header (1 line)**: brand · hostname · VM summary
- **Sidebar (~26 cols)**: VM list with status dots; selected item
  highlighted; filter input on top when `/` is active; `[+] create`
  at the bottom of the list
- **Content pane (rest)**: contextual content
  - `Detail` — selected VM's status/IP/name + scrollable dominfo
  - `CreateForm` — inline form (no modal)
  - `Help` — full-pane keybindings reference
  - `EmptyState` — friendly "no VMs yet" message
- **Status bar (2 lines)**: contextual key hints on line 1, refresh
  ticker + toast on line 2. Delete confirm renders here too (`[y]es /
  [n]o`) — no modal.

Modes drive both the right pane (`ui::draw_content`) and the keymap
(`events::map_key`). The result: state, IP, CPU time, etc. update live
on the 2-second tick without the user reopening anything. No more
modal popups for inspect, vnc, delete.

The single file that owns terminal state remains `src/tui/mod.rs`.
Every other file is pure logic / pure render and unit-tests cleanly.

### Why a TUI exists at all

Section 2 used to list "Web UI or TUI" as out of scope. The reversal came
out of a real diagnostic on the `aether` host: `qvm vnc <name>` was
printing `vncviewer <bind>:5900` (port form), which almost every modern
VNC client treats as **display 5900 → port 11800**. So qvm had been
silently handing users a broken connect string for months. We fixed the
string, but the bigger lesson was that the no-interactive stance was
hurting people: the user who hit this bug never even noticed the
"display vs port" subtlety because they had no easy way to *see* their
running VMs at a glance, watch state changes, or jump to a console
without remembering subcommand names.

The CLI is still the source of truth. The TUI in `src/tui/` is a thin
presenter — every action delegates to `commands::*` functions. No logic
duplication, no parallel state, no extra config. The CLI works exactly
as it did before. Adding the TUI was a smaller change than the previous
"don't add it" stance had implied.

We still draw a hard line at a **web UI**. That stays out of scope.

### Why UEFI for Alpine only

Alpine's BIOS-bootable nocloud cloud image has a known issue where it hangs
at "Loading initramfs-virt" inside libvirt due to syslinux/SeaBIOS quirks.
The UEFI variant boots correctly. Ubuntu, Debian, Fedora, Rocky all boot
fine on BIOS, so we don't pay the UEFI NVRAM-management overhead for them.
The `uefi` flag in the distro registry encodes this per-distro; create.rs
adds `--machine q35 --boot uefi,loader.secure=no` when it's set.

### Why `--osinfo name=X,require=off` instead of `--os-variant X`

Older `--os-variant` errors if the osinfo-db on the host doesn't have the
exact value (e.g. on hosts with older libosinfo packages). `--osinfo
name=X,require=off` tells virt-install "use X if you know it, otherwise
proceed with generic settings". We never error on unknown variants;
hypervisor tuning hints are not worth a creation failure.

### Why we delete with `virsh undefine --nvram` first, plain `undefine` fallback

UEFI VMs have an NVRAM file libvirt refuses to leave behind: `undefine` fails
unless `--nvram` is given. Real-world consequence: in the bash predecessor,
delete operations silently failed for UEFI VMs, leaving "ghost" VMs in
`virsh list --all` after the disks were gone. We:

1. Try `virsh undefine --nvram` (handles both BIOS and UEFI on any libvirt
   recent enough to recognise the flag).
2. If that fails (very old libvirt), fall back to plain `undefine` (BIOS only).
3. Remove the qcow2 / cloud-init seed / sidecar dir ourselves — we manage
   those files, we don't ask `--remove-all-storage` to do it. (It errors
   on non-pool-managed storage, which is fragile.)
4. Verify the domain is actually gone afterward and warn loudly if not.

---

## 6. Test strategy

We deliberately do not require libvirt or genisoimage in CI. The test
philosophy:

- **What we test:** validation, config parsing & layering, cloud-init
  user-data structure, distro registry invariants, sidecar file IO.
- **What we don't test in unit tests:** anything that requires running a
  real VM. There's no value in mocking virsh extensively; it would test the
  mock more than the code. Instead, the public CI verifies the binary
  builds, is statically linked, and `--help` runs.
- **Manual / on-host verification:** the README install steps double as a
  smoke test of the actual VM creation path.

### Running the tests

```
cargo test --release
```

32 tests across three files (`util_tests.rs`, `config_tests.rs`,
`cloudinit_tests.rs`). They run in milliseconds and require no privileges.

### Adding tests

If you add a new module with pure logic, add a `tests/<module>_tests.rs`
file at the top level. Don't put `#[cfg(test)]` blocks inside `src/`
modules; we expose the library surface specifically so all tests can be
external. This keeps test code out of the binary.

---

## 7. Build / release

Local development build:

```
cargo build --release
./target/release/qvm --help
```

Static, portable binary (what CI produces and what should be installed on
target hosts):

```
# amd64
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools
cargo build --release --target x86_64-unknown-linux-musl

# arm64 (cross from an amd64 host)
rustup target add aarch64-unknown-linux-musl
sudo apt install musl-tools gcc-aarch64-linux-gnu
# .cargo/config.toml: [target.aarch64-unknown-linux-musl] linker = "aarch64-linux-gnu-gcc"
cargo build --release --target aarch64-unknown-linux-musl
```

The CI in `.github/workflows/build.yml` is a matrix that builds **both**
`qvm-linux-amd64-static` and `qvm-linux-arm64-static` artifacts on every
push and PR. A separate cleanup job prunes everything but the newest two
artifacts per name, so storage doesn't grow unbounded.

CI deliberately does **not** create a GitHub release — that's a manual
decision for now.

---

## 8. What this tool will look like wrong if you don't read this file

A few things that look like missed opportunities but are intentional:

- **`qvm vnc` prints info; doesn't proxy a connection.** Adding a TLS
  proxy or generic websocket bridge would be a real feature; out of
  scope. The default is to bind VNC to 127.0.0.1 and tunnel via SSH.
  The connect string is printed in both `host:display` (canonical) and
  `host::port` (explicit) forms because viewers disagree on which they
  accept. `qvm vnc --browser` is the one exception — it spawns
  websockify + noVNC because rendering a framebuffer is the one thing
  the browser does better than a terminal.
- **`qvm init` is INTERACTIVE.** It launches the TUI onboarding wizard
  (`src/tui/onboard.rs`). The text-mode wizard that used to live here
  is gone — feedback was that it was the worst onboarding the user had
  seen. `--yes` writes default config silently for automation;
  `--force` overwrites an existing config.
- **`qvm` with no subcommand opens the TUI.** Proxmox-style split-pane
  with sidebar + contextual content + status bar. The refresh worker
  runs on a background thread (mpsc channel) so the UI never blocks on
  virsh. See section 5's "Why the TUI was redesigned" for the design
  history.
- **`qvm web` was tried and removed.** A full server-rendered management
  UI shipped briefly (commits 694e16d–acf3603) but the TUI was the
  better home for management. `qvm vnc --browser` covers the only
  case where a browser actually wins — rendering the VM framebuffer.
  See section 5's "Why the web management UI was removed".
- **Cross-arch import is refused, not emulated.** The qcow2 disk holds
  a kernel binary for a specific architecture; you cannot move an amd64
  guest to an arm64 host and expect it to boot. `qvm-meta.toml` records
  the source arch and import fails fast with a clear message pointing
  at "reinstall on the target arch". Cross-arch via qemu TCG emulation
  is technically possible but performance is unusable for real work.
- **`qvm export` live mode uses `blockcommit --pivot`.** If pivot fails
  mid-way the VM is left running on the overlay file; we don't
  silently recover. The error tells the user exactly which file the VM
  is on and how to merge manually. Defaulting to silent recovery would
  hide a degraded-state bug that needs eyeballs.

---

## 9. Operator-facing utility commands

Two commands exist purely to make the host setup experience humane:

### `qvm doctor` / `qvm doctor --install`

Checks the 5 external binaries qvm depends on (virsh, virt-install,
qemu-img, the right qemu-system-* for the host arch via
`arch::qemu_system_bin()`, genisoimage) and verifies libvirtd is
reachable. If anything is missing:

- Without `--install`: prints the exact install command for the host
  distro (apt-get / dnf / apk / pacman, detected via `/etc/os-release`
  `ID` and `ID_LIKE`).
- With `--install`: confirms with the user, then runs the install
  command, then re-runs the check.

`doctor` and `completions` are the only two commands that can run as
non-root, because they're diagnostic / informational.

### `qvm completions <shell>`

Prints a shell completion script generated by `clap_complete`. Supports
bash, zsh, fish, elvish, powershell. We print install hints to stderr and
the script to stdout, so `qvm completions bash | sudo tee
/etc/bash_completion.d/qvm` works cleanly.

The completion is regenerated from the CLI definition every invocation,
so it can never drift out of sync with the actual command surface.

## 10. Two test suites, deliberately split

This project has two **separate** test suites that test different things
and run in different places. Don't merge them.

### `tests/` (workspace root) — unit tests, run by `cargo test`

- Tests pure Rust logic only: validation, config parsing/layering,
  cloud-init YAML structure, sidecar IO, distro registry invariants.
- 32 tests across 3 files, all run in milliseconds.
- No libvirt, no genisoimage, no network required.
- Runs in CI on every push.

### `integration/` — bash smoke tests, run manually on a real KVM host

- Bash scripts that exercise the installed `qvm` binary against a real
  libvirt host with KVM.
- Actually create VMs, wait for them to boot, ssh in, delete them.
- See `integration/CLAUDE.md` for the full design and contribution rules.
- Each script is self-contained and cleans up its own VMs (success or
  failure, via trap EXIT).
- **Not run in CI.** Run them after installing on a new host, or after
  upgrading libvirt/qemu/kernel, or after touching create/delete/cloudinit.
- The most important one is `05-self-contained.sh`: it confirms VM disks
  have NO backing file. If this regresses, we're back to the Dev/Hermes
  disaster mode.

If you're tempted to merge these two suites, don't. The split is
deliberate: `cargo test` (in `tests/`) should be fast and side-effect-
free; the bash smoke tests (in `integration/`) should be honest about
what they need.

---

## 11. If you're an AI assistant editing this code

A few things to know:

- The bash predecessor is in the repo history; don't restore it.
- Don't add backing-file qcow2 creation under any circumstances. If a
  contributor asks for "thin VM clones", say no and explain section 1.
- Don't add `anyhow` or `color-eyre`. The two-variant Error works; keep
  it.
- Don't add `tokio` or async. Nothing in this tool is I/O-bound enough to
  benefit, and async runtimes balloon binary size.
- Don't add a daemon. There is no daemon. libvirtd is the daemon.
- Keep the binary single-file. No plugins, no script dirs, no add-ons.
- If you're considering a feature, ask: "Is this in the scope list in
  section 2?" If not, propose it as a separate tool.
