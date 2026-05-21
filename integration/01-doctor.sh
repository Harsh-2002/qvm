#!/usr/bin/env bash
# 01-doctor.sh - host dependency check must pass with everything installed.

set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

require_root
require_qvm

# qvm doctor returns 0 only when all deps present.
if qvm doctor >/dev/null 2>&1; then
    pass "qvm doctor reports no missing dependencies"
else
    note "qvm doctor output:"
    qvm doctor || true
    fail "qvm doctor reports missing deps - install them (or run 'qvm doctor --install') before continuing"
fi

# Verify libvirtd is actually reachable.
if virsh -c qemu:///system list >/dev/null 2>&1; then
    pass "libvirtd reachable on qemu:///system"
else
    fail "libvirtd not reachable. systemctl enable --now libvirtd"
fi
