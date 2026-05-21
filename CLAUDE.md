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

- Create / start / stop / restart / kill / delete VMs
- List, inspect, get IP, get an SSH command
- Pull and list distro base images (5 baked in, more via config)
- VNC connection info (replaces the need for Cockpit for graphical access)
- Resource changes (CPU, RAM, disk grow)
- One-time setup: `qvm init` writes a sample config and downloads the
  baseline image set
- A single static `amd64` binary; no daemon, no extra runtime

### Explicitly out of scope

These are excluded by design, not by accident. Do **not** add them without
re-reading section 1 and considering whether the tool is sprawling.

- Multi-host / clustering / federation
- Live migration
- Snapshots beyond what `virsh snapshot-*` already does (a thin passthrough
  would be acceptable; we won't reinvent it)
- Storage pools / LVM / Ceph / anything other than qcow2 files in a
  configurable directory
- Web UI or TUI (we're replacing Cockpit; adding another would defeat the
  point)
- User management or multi-tenant access (single root-owned tool)
- Image building or Packer-style workflows
- Container support (this is a VM tool)
- Automatic mirror selection / geo-pinning

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
├── main.rs            CLI dispatch (clap). Trivial - all logic in commands/.
├── lib.rs             Library surface so the test suite can exercise modules.
├── error.rs           One Result type. Two variants: User (printed plain),
│                      Command (with stderr). No anyhow soup.
├── cmd.rs             The ONLY layer that touches external processes.
│                      run() / run_inherit() / require() / exec().
├── libvirt.rs         Thin virsh wrapper. exists/is_running/start/stop/...
│                      ipv4() looks via agent, then DHCP lease, then ARP.
│                      undefine() handles UEFI NVRAM correctly.
├── config.rs          TOML schema, baked-in defaults, distro registry.
│                      Five built-in distros. User config layers over top.
├── cloudinit.rs       Seed generator. write_files() then build_iso().
│                      Split so tests can exercise file generation without
│                      needing genisoimage.
├── util.rs            Validation (vm name, username), random username,
│                      SHA-512 password hashing.
└── commands/
    ├── init.rs        First-run: write config, prep dirs, optionally pull
    │                  all images.
    ├── pull.rs        Atomic image download (write to .partial, mv on
    │                  success).
    ├── create.rs      The interesting one. Reads distro, generates seed,
    │                  full-copies the base, runs virt-install --import.
    ├── delete.rs      Stops, undefines (handles UEFI nvram), removes files,
    │                  verifies, reports honestly if libvirt still has it.
    ├── lifecycle.rs   start/stop/restart/kill - one-line virsh passthroughs.
    ├── info.rs        ls / inspect / ip / ssh-cmd.
    ├── images.rs      distros / images listings.
    ├── resources.rs   set-cpu / set-ram / resize-disk.
    └── vnc.rs         Prints the connect string with ssh -L tunnel guide.
                       --open tries to launch a local viewer.
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
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools     # on Debian/Ubuntu build hosts
cargo build --release --target x86_64-unknown-linux-musl
file target/x86_64-unknown-linux-musl/release/qvm
# -> "static-pie linked"
```

The CI in `.github/workflows/build.yml` runs the test suite and produces a
`qvm-linux-amd64-static` artifact on every push and PR. It deliberately
does **not** create a GitHub release — that's a manual decision for now.

---

## 8. What this tool will look like wrong if you don't read this file

A few things that look like missed opportunities but are intentional:

- **No `qvm console` command yet.** `virsh console <name>` works; we'll add
  a thin wrapper, but the priority was lifecycle + image management first.
- **`qvm vnc` prints info; doesn't proxy a connection.** Adding a TLS
  proxy or websocket bridge would be a real feature; not in scope. The
  default is to bind VNC to 127.0.0.1 and tunnel via SSH.
- **No `qvm export` / `qvm import` yet.** These are the qemu-nbd-based
  data-extraction commands the user did manually during the disaster
  recovery. They're high-value future additions. The pattern (mount
  via qemu-nbd, rsync /root /home /opt /srv /var/lib/docker/volumes
  /etc-selected) is well-tested manually.
- **No `qvm flatten` command yet.** This would detach an existing
  overlay-based VM from its backing file (the recovery operation for VMs
  created by the old bash tool). For now, the equivalent manual command is
  documented in the README.
- **No interactive wizard.** The bash predecessor had one. The Rust version
  doesn't because the CLI is short and docker-style; if the user can type
  `docker run` they can type `qvm run`. We can add a wizard later if anyone
  complains.

---

## 9. Operator-facing utility commands

Two commands exist purely to make the host setup experience humane:

### `qvm doctor` / `qvm doctor --install`

Checks all 6 external binaries qvm depends on (virsh, virt-install,
qemu-img, qemu-system-x86_64, genisoimage, wget) and verifies libvirtd is
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

### `test/` (single 't', not plural) — integration tests, run manually on real host

- Bash scripts that exercise the installed `qvm` binary against a real
  libvirt host with KVM.
- Actually create VMs, wait for them to boot, ssh in, delete them.
- See `test/CLAUDE.md` for the full design and contribution rules.
- Each script is self-contained and cleans up its own VMs (success or
  failure, via trap EXIT).
- **Not run in CI.** Run them after installing on a new host, or after
  upgrading libvirt/qemu/kernel, or after touching create/delete/cloudinit.
- The most important one is `05-self-contained.sh`: it confirms VM disks
  have NO backing file. If this regresses, we're back to the Dev/Hermes
  disaster mode.

If you're tempted to merge these two suites, don't. The split is
deliberate: `cargo test` should be fast and side-effect-free; integration
tests should be honest about what they need.

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
