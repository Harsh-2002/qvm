# qvm

Thin, opinionated KVM/libvirt CLI + TUI for a single host. No daemon.
Self-contained VM disks (no qcow2 overlay corruption).

- Mouse-aware terminal UI · in-browser VNC console · auto-pulls images
- 5 cloud distros built in (Ubuntu 24.04, Debian 13, Fedora 42, Alpine 3.20, Rocky 9)
- A single static `amd64` binary

## Install

```
curl -fsSL https://raw.githubusercontent.com/Harsh-2002/qvm/main/install.sh | sudo sh
```

Installs the latest static binary to `/usr/local/bin/qvm`, brings in
required host packages (`virsh`, `virt-install`, `qemu-img`,
`genisoimage`, `wget`) via your package manager, and sets up shell
completions.

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

## Docs

- **`ARCHITECTURE.md`** — design, invariants, lifecycle, full module map
- **`qvm --help`** — every command + flag (clap-generated, always current)
- **`sudo qvm doctor`** — check (and optionally install) host dependencies

## License

MIT.
