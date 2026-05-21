#!/usr/bin/env bash
# run-all.sh - run every integration test, summarise.

set -uo pipefail
. "$(dirname "$0")/lib.sh"

require_root

cd "$(dirname "$0")"

declare -a passed=() failed=() skipped=()
for t in 0*.sh; do
    [ -f "$t" ] || continue
    printf '\n========== %s ==========\n' "$t"
    if bash "./$t"; then
        passed+=("$t")
    else
        rc=$?
        if [ $rc -eq 77 ]; then
            skipped+=("$t")
        else
            failed+=("$t")
        fi
    fi
done

echo
echo "=================================================="
echo "Summary"
echo "=================================================="
echo "passed:  ${#passed[@]}"
for t in "${passed[@]:-}";  do [ -n "$t" ] && echo "  + $t"; done
echo "skipped: ${#skipped[@]}"
for t in "${skipped[@]:-}"; do [ -n "$t" ] && echo "  - $t"; done
echo "failed:  ${#failed[@]}"
for t in "${failed[@]:-}";  do [ -n "$t" ] && echo "  ! $t"; done

[ "${#failed[@]}" -eq 0 ] || exit 1
