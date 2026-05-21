#!/usr/bin/env bash
# Shared helpers for qvm integration tests.
# Source this from each test script: . "$(dirname "$0")/lib.sh"

set -euo pipefail

# --- output ---
RED=$'\033[0;31m'; GREEN=$'\033[0;32m'; YELLOW=$'\033[0;33m'; RESET=$'\033[0m'

pass() { printf '%sPASS:%s %s\n' "$GREEN" "$RESET" "$*"; }
fail() { printf '%sFAIL:%s %s\n' "$RED"   "$RESET" "$*" >&2; exit 1; }
skip() { printf '%sSKIP:%s %s\n' "$YELLOW" "$RESET" "$*" >&2; exit 77; }
note() { printf '       %s\n' "$*"; }

# --- preconditions ---

require_root() {
    [ "$(id -u)" -eq 0 ] || fail "must run as root"
}

require_qvm() {
    command -v qvm >/dev/null 2>&1 || fail "qvm not in PATH; install it first"
}

require_kvm() {
    [ -e /dev/kvm ] || skip "no /dev/kvm; cannot run guests"
    [ -r /dev/kvm ] || skip "/dev/kvm not readable"
}

require_bridge() {
    local br
    br="$(awk -F'=' '/^bridge/{gsub(/["[:space:]]/,"",$2); print $2}' \
        /etc/qvm/config.toml 2>/dev/null || true)"
    br="${br:-br0}"
    ip link show "$br" >/dev/null 2>&1 \
        || skip "configured bridge '$br' does not exist on this host"
}

# --- VM lifecycle helpers ---

# Wait for a VM's IP to appear (qemu-guest-agent + DHCP). 180s default.
wait_for_ip() {
    local name="$1" timeout="${2:-180}" t=0
    while [ $t -lt "$timeout" ]; do
        if qvm ip "$name" >/dev/null 2>&1; then
            qvm ip "$name" | awk '/ipv4/{print $4}' | cut -d/ -f1 | head -1
            return 0
        fi
        sleep 5
        t=$((t + 5))
    done
    return 1
}

# Wait for SSH port to be open on a VM's IP.
wait_for_ssh() {
    local name="$1" timeout="${2:-180}" t=0 ip
    ip="$(wait_for_ip "$name" "$timeout")" || return 1
    while [ $t -lt "$timeout" ]; do
        if (echo > /dev/tcp/"$ip"/22) >/dev/null 2>&1; then
            echo "$ip"
            return 0
        fi
        sleep 3
        t=$((t + 3))
    done
    return 1
}

# Force-cleanup a VM, swallowing all errors. Idempotent.
nuke() {
    local name="$1"
    qvm kill "$name" >/dev/null 2>&1 || true
    qvm rm "$name" -f >/dev/null 2>&1 || true
}

# Register a VM for cleanup on script exit (success or failure).
cleanup_on_exit() {
    local name="$1"
    # shellcheck disable=SC2064
    trap "nuke '$name'" EXIT
}
