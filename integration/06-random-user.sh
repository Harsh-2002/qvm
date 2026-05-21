#!/usr/bin/env bash
# 06-random-user.sh - omitting -u must generate a random vm<6 chars> username,
# persist it, and have ssh-cmd recover it correctly.

set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

require_root
require_qvm
require_kvm
require_bridge

NAME="qvmtest-06-randu"
cleanup_on_exit "$NAME"
nuke "$NAME"

qvm pull debian:13 >/dev/null 2>&1 || true
out="$(qvm run "$NAME" debian:13 -c 1 -m 1 -s 8 --no-autostart 2>&1)"

# Must mention "Generated user: vm..."
gen="$(echo "$out" | sed -n 's/.*Generated user: \(vm[a-z0-9]\{6\}\).*/\1/p' | head -1)"
if [ -n "$gen" ]; then
    pass "create printed generated user: $gen"
else
    fail "no 'Generated user' line found. full output: $out"
fi

# Must be persisted to .vmuser
recorded="$(tr -d '[:space:]' < /var/lib/qvm/cloudinit/"$NAME"/.vmuser 2>/dev/null || true)"
if [ "$recorded" = "$gen" ]; then
    pass ".vmuser sidecar matches: $recorded"
else
    fail ".vmuser mismatch (sidecar=$recorded, gen=$gen)"
fi

# Once booted, ssh-cmd must print exactly that user.
note "waiting for boot..."
ip="$(wait_for_ssh "$NAME" 180)" || fail "VM never reached SSH"
sshline="$(qvm ssh-cmd "$NAME")"
if [ "$sshline" = "ssh $gen@$ip" ]; then
    pass "ssh-cmd recovers generated user correctly: $sshline"
else
    fail "ssh-cmd mismatch: $sshline (expected: ssh $gen@$ip)"
fi
