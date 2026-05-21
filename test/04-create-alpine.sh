#!/usr/bin/env bash
# 04-create-alpine.sh - UEFI path. Alpine cloud image requires UEFI.

set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

require_root
require_qvm
require_kvm
require_bridge

# OVMF must be installed for UEFI guests.
have_ovmf=0
for d in /usr/share/OVMF /usr/share/edk2 /usr/share/qemu/firmware; do
    [ -e "$d" ] && have_ovmf=1
done
[ $have_ovmf -eq 1 ] || skip "no OVMF/edk2 firmware found; install 'ovmf'"

NAME="qvmtest-04-alpine"
cleanup_on_exit "$NAME"
nuke "$NAME"

qvm pull alpine:3.20 >/dev/null 2>&1 || true
qvm run "$NAME" alpine:3.20 -c 1 -m 1 -s 8 --no-autostart >/dev/null

if qvm ls | grep -q "$NAME"; then pass "alpine VM listed"
else fail "alpine VM missing from ls"
fi

note "waiting for SSH (up to 180s, UEFI boot is slower)..."
ip="$(wait_for_ssh "$NAME" 180)" \
    || fail "alpine UEFI VM never reached SSH - check /var/log/libvirt/qemu/${NAME}.log"
pass "alpine UEFI VM booted at $ip"

# Verify the domain has a UEFI NVRAM file (the bug the bash predecessor hit).
if virsh dumpxml "$NAME" | grep -qi 'nvram'; then
    pass "UEFI NVRAM provisioned"
else
    fail "expected NVRAM in dumpxml but found none"
fi

# Delete must handle NVRAM cleanup.
qvm rm "$NAME" -f >/dev/null
if qvm ls | grep -q "$NAME"; then
    fail "alpine VM still defined after rm (NVRAM bug regressed)"
else
    pass "alpine UEFI VM cleanly deleted (NVRAM included)"
fi
