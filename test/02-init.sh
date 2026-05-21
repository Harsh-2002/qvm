#!/usr/bin/env bash
# 02-init.sh - qvm init creates config, prepares directories, distros are listed.

set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

require_root
require_qvm

# If a config already exists, leave it alone - init should be idempotent.
qvm init >/dev/null 2>&1
if [ -f /etc/qvm/config.toml ]; then
    pass "config exists at /etc/qvm/config.toml"
else
    fail "qvm init did not create /etc/qvm/config.toml"
fi

# Distros should print at least the 5 baked-in ones.
n="$(qvm distros | tail -n +2 | wc -l)"
if [ "$n" -ge 5 ]; then
    pass "distros listed (>=5 entries: $n)"
else
    fail "expected >=5 distros, got $n"
fi

# Data dirs created.
for d in /var/lib/qvm/images /var/lib/qvm/vms /var/lib/qvm/cloudinit; do
    if [ -d "$d" ]; then pass "data dir present: $d"
    else fail "missing data dir: $d"
    fi
done
