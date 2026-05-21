# qvm integration tests

Manual tests that exercise the real `qvm` binary against a live libvirt host.

Run on any KVM-capable Linux host as root:

```bash
sudo ./run-all.sh
```

Or individual tests:

```bash
sudo ./01-doctor.sh
sudo ./03-create-debian.sh
```

Each script creates one or more VMs named `qvmtest-*` and cleans them up on
exit (even on failure). They take a few minutes each because they actually
boot guests.

See `CLAUDE.md` for the design and contribution rules.
