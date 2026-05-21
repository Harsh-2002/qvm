#!/usr/bin/env bash
# 05-self-contained.sh - VM disks MUST NOT have a backing file.
#
# This is THE regression test. If this ever fails, the Dev/Hermes
# disaster can happen again.

set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

require_root
require_qvm
require_kvm
require_bridge

NAME="qvmtest-05-flat"
cleanup_on_exit "$NAME"
nuke "$NAME"

qvm pull debian:13 >/dev/null 2>&1 || true
qvm run "$NAME" debian:13 -c 1 -m 1 -s 8 --no-autostart >/dev/null

disk="/var/lib/qvm/vms/${NAME}.qcow2"
[ -f "$disk" ] || fail "disk file does not exist: $disk"

info="$(qemu-img info "$disk")"
note "$info"

# Critical assertion: there must be NO 'backing file' line.
if echo "$info" | grep -qi 'backing file'; then
    fail "VM disk has a backing file! Self-contained disks rule violated."
fi
pass "VM disk is self-contained (no backing file)"

# Belt-and-suspenders: delete the base image and confirm VM still boots.
# (We skip the boot test for time and just check the disk is still
# fully usable via qemu-img check.)
if qemu-img check "$disk" >/dev/null 2>&1; then
    pass "qemu-img check passes"
else
    fail "qcow2 disk did not pass qemu-img check"
fi
