#!/usr/bin/env bash
# 03-create-debian.sh - full create + boot + ssh + rm on debian:13 (BIOS path).

set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

require_root
require_qvm
require_kvm
require_bridge

NAME="qvmtest-03-debian"
cleanup_on_exit "$NAME"
nuke "$NAME"   # make sure no leftover

qvm pull debian:13 >/dev/null 2>&1 || true   # may already be present
qvm run "$NAME" debian:13 -c 1 -m 1 -s 8 --no-autostart >/dev/null

# It should appear in `qvm ls`.
if qvm ls | grep -q "$NAME"; then pass "VM '$NAME' is listed"
else fail "VM not listed"
fi

# Wait for boot + DHCP.
note "waiting for SSH (up to 180s)..."
ip="$(wait_for_ssh "$NAME" 180)" || fail "VM never reached SSH"
pass "VM booted and SSH reachable at $ip"

# ssh-cmd should print the right user@ip.
sshline="$(qvm ssh-cmd "$NAME")"
if echo "$sshline" | grep -qE "^ssh vm[a-z0-9]{6}@$ip$"; then
    pass "ssh-cmd prints expected 'ssh vm<rand>@<ip>'"
else
    fail "ssh-cmd output unexpected: $sshline"
fi

# Delete and verify it's gone.
qvm rm "$NAME" -f >/dev/null
if qvm ls | grep -q "$NAME"; then fail "VM still listed after rm"
else pass "VM cleanly deleted"
fi

# Files removed.
if [ -f /var/lib/qvm/vms/"$NAME".qcow2 ]; then fail "qcow2 disk left behind"
else pass "disk file removed"
fi
