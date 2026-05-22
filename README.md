# qvm

Thin, opinionated KVM/libvirt CLI + TUI for a single host. No daemon.
Self-contained VM disks (no qcow2 overlay corruption).

- Keyboard-driven terminal UI · in-browser VNC console · auto-pulls images
- 10 cloud distros built in — full amd64 + arm64 coverage
- A single static binary per arch

## Install

```
curl -fsSL https://raw.githubusercontent.com/Harsh-2002/qvm/main/install.sh | sudo sh
```

Installs the latest static binary to `/usr/local/bin/qvm`, brings in
required host packages (`virsh`, `virt-install`, `qemu-img`,
`genisoimage`) via your package manager, and sets up shell completions.
HTTPS image downloads are handled by the binary itself — no `wget` /
`curl` required at runtime.

## Use

```
sudo qvm                                # opens the TUI (or onboarding on first run)
sudo qvm run web01 debian:13 -u me -p 'change-this'
sudo qvm ssh-cmd web01
sudo qvm vnc web01 --browser            # opens a noVNC bridge + QR for mobile
sudo qvm rm web01
```

Username and password are required at create time; qvm intentionally
has no default for either. CPU / RAM / disk default to 2 / 4 GB / 50
GB and can be overridden with `-c` / `-m` / `-s`.

## Supported distros

Every entry below ships with stable amd64 **and** arm64 cloud-init
NoCloud images (except Arch, which is x86_64-only upstream). The URLs
are the exact sources `qvm pull <key>` downloads from — click through
to verify.

| Distro | Key | Firmware | amd64 | arm64 |
|---|---|---|---|---|
| ![Ubuntu](https://img.shields.io/badge/Ubuntu-24.04%20LTS-E95420?logo=ubuntu&logoColor=white) | `ubuntu:24.04` | BIOS | [img](https://cloud-images.ubuntu.com/releases/noble/release/ubuntu-24.04-server-cloudimg-amd64.img) | [img](https://cloud-images.ubuntu.com/releases/noble/release/ubuntu-24.04-server-cloudimg-arm64.img) |
| ![Ubuntu](https://img.shields.io/badge/Ubuntu-26.04%20LTS-E95420?logo=ubuntu&logoColor=white) | `ubuntu:26.04` | BIOS | [img](https://cloud-images.ubuntu.com/releases/26.04/release/ubuntu-26.04-server-cloudimg-amd64.img) | [img](https://cloud-images.ubuntu.com/releases/26.04/release/ubuntu-26.04-server-cloudimg-arm64.img) |
| ![Debian](https://img.shields.io/badge/Debian-13%20trixie-A81D33?logo=debian&logoColor=white) | `debian:13` | BIOS | [qcow2](https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2) | [qcow2](https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-arm64.qcow2) |
| ![Fedora](https://img.shields.io/badge/Fedora-42-294172?logo=fedora&logoColor=white) | `fedora:42` | BIOS | [qcow2](https://download.fedoraproject.org/pub/fedora/linux/releases/42/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-42-1.1.x86_64.qcow2) | [qcow2](https://download.fedoraproject.org/pub/fedora/linux/releases/42/Cloud/aarch64/images/Fedora-Cloud-Base-Generic-42-1.1.aarch64.qcow2) |
| ![Alpine](https://img.shields.io/badge/Alpine-3.20-0D597F?logo=alpinelinux&logoColor=white) | `alpine:3.20` | UEFI | [qcow2](https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/cloud/nocloud_alpine-3.20.3-x86_64-uefi-cloudinit-r0.qcow2) | [qcow2](https://dl-cdn.alpinelinux.org/alpine/v3.20/releases/cloud/nocloud_alpine-3.20.3-aarch64-uefi-cloudinit-r0.qcow2) |
| ![Rocky](https://img.shields.io/badge/Rocky-9-10B981?logo=rockylinux&logoColor=white) | `rocky:9` | BIOS | [qcow2](https://download.rockylinux.org/pub/rocky/9/images/x86_64/Rocky-9-GenericCloud-Base.latest.x86_64.qcow2) | [qcow2](https://download.rockylinux.org/pub/rocky/9/images/aarch64/Rocky-9-GenericCloud-Base.latest.aarch64.qcow2) |
| ![AlmaLinux](https://img.shields.io/badge/AlmaLinux-9-0B5FA0?logo=almalinux&logoColor=white) | `almalinux:9` | BIOS | [qcow2](https://repo.almalinux.org/almalinux/9/cloud/x86_64/images/AlmaLinux-9-GenericCloud-latest.x86_64.qcow2) | [qcow2](https://repo.almalinux.org/almalinux/9/cloud/aarch64/images/AlmaLinux-9-GenericCloud-latest.aarch64.qcow2) |
| ![openSUSE](https://img.shields.io/badge/openSUSE-Leap%2015.6-73BA25?logo=opensuse&logoColor=white) | `opensuse:15.6` | BIOS | [qcow2](https://download.opensuse.org/distribution/leap/15.6/appliances/openSUSE-Leap-15.6-Minimal-VM.x86_64-15.6.0-Cloud-Build19.146.qcow2) | [qcow2](https://download.opensuse.org/distribution/leap/15.6/appliances/openSUSE-Leap-15.6-Minimal-VM.aarch64-15.6.0-Cloud-Build19.146.qcow2) |
| ![CentOS](https://img.shields.io/badge/CentOS%20Stream-10-262577?logo=centos&logoColor=white) | `centos-stream:10` | BIOS | [qcow2](https://cloud.centos.org/centos/10-stream/x86_64/images/CentOS-Stream-GenericCloud-10-latest.x86_64.qcow2) | [qcow2](https://cloud.centos.org/centos/10-stream/aarch64/images/CentOS-Stream-GenericCloud-10-latest.aarch64.qcow2) |
| ![Arch](https://img.shields.io/badge/Arch%20Linux-rolling-1793D1?logo=archlinux&logoColor=white) | `arch` | BIOS | [qcow2](https://geo.mirror.pkgbuild.com/images/latest/Arch-Linux-x86_64-cloudimg.qcow2) | — |

Want a distro not listed? Add a `[distros."your:key"]` block to
`/etc/qvm/config.toml` — no code change required (see
`ARCHITECTURE.md` §8).

## Docs

- **`ARCHITECTURE.md`** — design, invariants, lifecycle, full module map
- **`qvm --help`** — every command + flag (clap-generated, always current)
- **`sudo qvm doctor`** — check (and optionally install) host dependencies

## License

MIT.
