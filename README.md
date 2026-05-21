# qvm

A thin, opinionated CLI for managing KVM/libvirt VMs — docker-style commands, zero daemon, zero overlay corruption.

```
qvm run web01 ubuntu:24.04 -c 4 -m 8 -s 100
qvm ls
qvm ssh-cmd web01
```

---

## Why this exists

The predecessor was a bash script that created VMs as qcow2 **overlays** on a shared base image.
When `vm pull ubuntu24` was later run, every VM reading unchanged blocks from that base silently
got a different kernel, different libraries, different config files.
Two production VMs required manual recovery via `qemu-nbd`.

**qvm fixes this architecturally:** every VM disk is a full `qemu-img convert` copy of the base.
Pulling a new base image after the fact changes nothing for existing VMs — ever.

---

## Requirements

- Linux host with KVM enabled
- `virsh` / `virt-install` (libvirt)
- `qemu-img` / `qemu-system-x86_64` (qemu)
- `genisoimage`
- `wget`
- A network bridge (`br0` by default)

Run `sudo qvm doctor` to check everything at once.

---

## Install

```bash
git clone https://github.com/Harsh-2002/qvm && cd qvm
cargo build --release
sudo install -m 0755 target/release/qvm /usr/local/bin/qvm
```

Or grab the static binary from [CI artifacts](.github/workflows/build.yml):

```bash
# The workflow produces qvm-linux-amd64-static on every push.
sudo install -m 0755 qvm-linux-amd64-static /usr/local/bin/qvm
```

### Static build (portable, no glibc dependency)

```bash
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools   # Debian/Ubuntu build hosts
cargo build --release --target x86_64-unknown-linux-musl
```

---

## First-run setup

```bash
# Check (and optionally install) all dependencies
sudo qvm doctor
sudo qvm doctor --install

# Write /etc/qvm/config.toml, create data dirs, download all 5 base images
sudo qvm init --pull-all
```

Edit `/etc/qvm/config.toml` to set your bridge, default CPU/RAM/disk, SSH keys, etc.

---

## Command reference

### VMs

| Command | Description |
|---------|-------------|
| `qvm run <name> [distro]` | Create and start a VM (alias: `create`) |
| `qvm ls` | List all VMs (alias: `ps`) |
| `qvm inspect <name>` | Show VM details |
| `qvm ip <name>` | Get IPv4 address |
| `qvm ssh-cmd <name>` | Print a ready-to-paste `ssh` command |
| `qvm start <name>` | Start a stopped VM |
| `qvm stop <name>` | Graceful shutdown |
| `qvm restart <name>` | Reboot (alias: `reboot`) |
| `qvm kill <name>` | Force power-off |
| `qvm rm <name>` | Delete VM and all its data (alias: `delete`) |
| `qvm vnc <name>` | Print VNC connection info |

### Images

| Command | Description |
|---------|-------------|
| `qvm distros` | List configured distros |
| `qvm images` | List downloaded base images |
| `qvm pull <distro>` | Download/refresh a distro image (atomic) |

### Resources

| Command | Description |
|---------|-------------|
| `qvm set-cpu <name> <n>` | Change vCPU count (reboot to apply) |
| `qvm set-ram <name> <gb>` | Change RAM in GB (reboot to apply) |
| `qvm resize-disk <name> <size>` | Grow disk (e.g. `+50G` or `200G`) |

### Host

| Command | Description |
|---------|-------------|
| `qvm doctor [--install]` | Check dependencies, optionally install them |
| `qvm completions <shell>` | Print shell completion script |
| `qvm init [--pull-all]` | First-run setup |

### Create flags

```
qvm run <name> [distro] [flags]

  -c, --cpus <N>       vCPUs (default: 2)
  -m, --memory <GB>    RAM in GB (default: 4)
  -s, --disk <GB>      Disk in GB (default: 50)
  -u, --user <name>    Login username (default: random vmXXXXXX)
  -p, --password <pw>  Plaintext password (hashed; default in config)
      --no-autostart   Do not autostart on host boot
```

---

## Built-in distros

| Tag | Notes |
|-----|-------|
| `ubuntu:24.04` | Noble — stable release |
| `debian:13` | Trixie — latest stable point release |
| `fedora:42` | Stable release |
| `alpine:3.20` | UEFI cloud image (BIOS hangs on this distro) |
| `rocky:9` | Rocky Linux 9 GenericCloud |

Add your own in config:

```toml
[distros."archlinux:2024"]
image  = "archlinux-2024.qcow2"
osinfo = "archlinux"
shell  = "/bin/bash"
uefi   = false
url    = "https://..."
```

---

## Configuration

`/etc/qvm/config.toml` — all fields optional, defaults baked into the binary.

```toml
[paths]
images    = "/var/lib/qvm/images"
vms       = "/var/lib/qvm/vms"
cloudinit = "/var/lib/qvm/cloudinit"

[network]
bridge = "br0"

[defaults]
distro    = "debian:13"
cpus      = 2
memory_gb = 4
disk_gb   = 50
autostart = true

[vnc]
bind = "127.0.0.1"   # "0.0.0.0" to expose on LAN

ssh_keys = [
    "ssh-ed25519 AAAA... you@host",
]
```

---

## VNC

```bash
qvm vnc web01
```

By default VNC binds to `127.0.0.1`. Tunnel from your laptop:

```bash
ssh -L 5901:127.0.0.1:5901 root@your-host
vncviewer 127.0.0.1:5901
```

---

## Design principles

1. **Every VM disk is a full copy.** `qemu-img convert`, never `-b`. Pulling a new base cannot corrupt existing VMs.
2. **libvirt is the source of truth.** No parallel state files. `qvm ls` is `virsh list --all` with formatting.
3. **No per-distro code branches.** All cross-distro behavior (GRUB, qemu-guest-agent, init system) is feature-detected at first boot by a generic shell script.
4. **Shell out, don't link.** No libvirt-rs FFI. Every action is a `virsh`/`qemu-img` call you can paste into a terminal.
5. **Single static binary.** No daemon, no plugin dir, no runtime.

---

## Tests

```bash
# Unit tests — pure logic, no libvirt, no root (32 tests, milliseconds)
cargo test --release

# Integration tests — real VMs on a real KVM host
sudo ./test/run-all.sh
```

The integration suite (`test/`) creates real VMs, verifies behavior, and cleans up after itself. See [`test/README.md`](test/README.md) for details. The most important test is `05-self-contained.sh` — it asserts that no VM disk has a backing file.

---

## What this tool deliberately does NOT do

- Multi-host / clustering / live migration
- Snapshots (use `virsh snapshot-*` directly)
- Web UI or TUI
- User management or multi-tenant access
- Storage pools / LVM / Ceph
- Image building / Packer-style workflows
- Container support

If you want one of these, fork it. A small tool that does its job precisely is more useful than one that does everything badly.

---

## License

MIT
