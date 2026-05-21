# test/CLAUDE.md

Manual integration tests for `qvm`.

These tests **must be run on a real KVM-capable host with libvirt installed**.
The unit tests in `tests/` (the cargo-level test suite) cover everything that
can be checked without a live hypervisor. Anything beyond that — actually
creating a VM, booting it, getting an IP, deleting it — lives here.

## Why these aren't in `cargo test`

1. They require root, KVM, a working bridge, network access for image pulls.
2. Each test takes minutes to complete (real boot times).
3. They mutate libvirt state. Running them in CI would be reckless.

`cargo test` is for catching code-level regressions. The scripts here are
for catching **environment-level regressions** before you trust `qvm` with
production VMs on a new host.

## When to run them

- After installing `qvm` on a new host (validates the host setup).
- After upgrading `libvirt`, `qemu`, or kernel on an existing host.
- Before relying on a new distro entry in the registry.
- After any non-trivial change to `commands/create.rs`, `commands/delete.rs`,
  or `cloudinit.rs`.

## What's here

```
test/
├── CLAUDE.md            this file
├── README.md            short user-facing version
├── run-all.sh           orchestrator: runs every test, summarises
├── 01-doctor.sh         host dependency check passes
├── 02-init.sh           `qvm init --yes` writes the default config
├── 03-create-debian.sh  create + boot + ssh + rm cycle on debian:13
├── 04-create-alpine.sh  same on alpine:3.20 (UEFI path)
├── 05-self-contained.sh prove the VM disk has NO backing file
├── 06-random-user.sh    CLI without -u generates random username
├── 07-completions.sh    completions are valid for bash/zsh/fish
└── lib.sh               shared helpers (assert/cleanup)
```

## Running

```bash
sudo ./test/run-all.sh
```

Or one at a time:

```bash
sudo ./test/01-doctor.sh
sudo ./test/03-create-debian.sh
```

Each script is **self-contained** and idempotent: starts with a known
state, ends by cleaning up the VMs it created (named `qvmtest-NN-...`).

## Conventions

- Every test VM is named `qvmtest-<NN>-<purpose>` so accidental leftovers
  are easy to grep and clean up.
- Tests delete what they create on success **and** on failure (trap EXIT).
- A test prints `PASS:` or `FAIL:` lines that `run-all.sh` greps for.
- Tests echo every external command via `set -x` for debuggability.
- Tests skip cleanly (exit 77) if the host can't support them — e.g.
  no KVM available, no bridge `br0`. Use `lib.sh:require_kvm`.

## What NOT to do here

- Don't unit-test pure Rust logic — that's `tests/` at the workspace root.
- Don't depend on a specific host distro family. Tests must work whether
  the host is Debian, Ubuntu, Fedora, or Arch (anything `qvm doctor`
  supports).
- Don't leave VMs behind on failure. Use `trap` to clean up.
- Don't hardcode IPs. Get them via `qvm ip`.
- Don't time out on boot — Alpine UEFI can take ~60s on slow hosts.
  Use `lib.sh:wait_for_ssh` (polls up to 180s).

## When a test fails

Read its stderr first; tests are verbose by design. If the failure looks
environmental:

1. Run `sudo qvm doctor` — usually catches the obvious one.
2. Confirm libvirtd is reachable: `virsh -c qemu:///system list`.
3. Confirm your bridge exists: `ip link show br0` (or whatever bridge is
   in `/etc/qvm/config.toml`).
4. Check `/var/log/libvirt/qemu/<vm-name>.log` for boot errors.

If the failure looks like a real qvm bug, capture:

- The full test output
- `qvm --version`
- Host distro (`cat /etc/os-release`)
- `virsh --version` / `qemu-img --version`

## Notes for AI assistants

- These are shell scripts, not Rust. Keep them POSIX-ish (bash is fine,
  zsh-isms are not). They run on every distro we claim to support.
- Don't add tests that need a specific guest distro to be running. Test
  the *qvm* behaviour, not the guest's.
- If you add a new test, name it `NN-purpose.sh` and add it to `run-all.sh`.
- Don't make tests depend on each other. Each one should set up and tear
  down its own VMs.
