#!/usr/bin/env bash
# 07-completions.sh - completion scripts generate and contain expected commands.

set -euo pipefail
# shellcheck source=lib.sh
. "$(dirname "$0")/lib.sh"

require_qvm   # no root needed for completions

for shell in bash zsh fish; do
    out="$(qvm completions "$shell" 2>/dev/null)"
    [ -n "$out" ] || fail "no output for $shell"

    # Each must reference our actual subcommand names.
    echo "$out" | grep -q 'create' || fail "$shell completion missing 'create'"
    echo "$out" | grep -q 'doctor' || fail "$shell completion missing 'doctor'"
    echo "$out" | grep -q 'rm'     || fail "$shell completion missing 'rm'"

    # Sanity-check the shell parses the script.
    case "$shell" in
        bash) echo "$out" | bash -n  || fail "bash completion failed syntax check" ;;
        zsh)
            if command -v zsh >/dev/null 2>&1; then
                echo "$out" | zsh -n - || fail "zsh completion failed syntax check"
            else
                note "zsh not installed; skipping syntax check (output still generated)"
            fi
            ;;
        fish)
            if command -v fish >/dev/null 2>&1; then
                echo "$out" | fish --no-execute - || fail "fish completion failed syntax check"
            else
                note "fish not installed; skipping syntax check (output still generated)"
            fi
            ;;
    esac
    pass "$shell completion generates and is syntactically valid"
done
