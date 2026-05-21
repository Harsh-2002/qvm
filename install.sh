#!/bin/sh
# install.sh — install or update qvm.
#
# POSIX shell. No bashisms. Verify with `shellcheck -s sh install.sh`.
#
# Four jobs, nothing more:
#   1. Install or update /usr/local/bin/qvm from the latest CI artifact.
#   2. Ensure host dependencies are present (via `qvm doctor --install`).
#   3. Detect the user's shell and drop in completions.
#   4. Print the source-reload command so completions are live immediately.
#
# Output is minimal on purpose. Configuration is NOT this script's job —
# run `sudo qvm` after install for the interactive onboarding wizard.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Harsh-2002/qvm/main/install.sh | sudo sh
#
# Tested on Debian, Ubuntu, Fedora, Alpine, Arch.
set -eu

REPO="Harsh-2002/qvm"
ARTIFACT_URL="https://nightly.link/${REPO}/workflows/build/main/qvm-linux-amd64-static.zip"
BIN_DIR="/usr/local/bin"
BIN="${BIN_DIR}/qvm"

die() { printf '\033[31m✗\033[0m %s\n' "$*" >&2; exit 1; }
ok()  { printf '\033[32m✓\033[0m %s\n' "$*"; }

# ── guards ────────────────────────────────────────────────────────────────────
[ "$(uname -s)" = "Linux"   ] || die "Linux only."
[ "$(uname -m)" = "x86_64"  ] || die "x86_64 only."
[ "$(id -u)"    -eq 0       ] || die "run as root (sudo sh install.sh)."
command -v curl  >/dev/null 2>&1 || die "curl missing."
command -v unzip >/dev/null 2>&1 || die "unzip missing."

ACTION="installed"
[ -x "$BIN" ] && ACTION="updated"

# ── 1. binary ─────────────────────────────────────────────────────────────────
TMP="$(mktemp -d)"
# shellcheck disable=SC2064
trap "rm -rf '$TMP'" EXIT
curl -fsSL -o "$TMP/qvm.zip" "$ARTIFACT_URL"
unzip -qo "$TMP/qvm.zip" -d "$TMP"
chmod +x "$TMP/qvm"
"$TMP/qvm" --version >/dev/null 2>&1 || die "binary failed sanity check."
install -m 0755 "$TMP/qvm" "$BIN"
ok "qvm $ACTION → $BIN"

# ── 2. host deps ──────────────────────────────────────────────────────────────
"$BIN" doctor --install --yes >/dev/null 2>&1 || true
ok "host dependencies checked"

# ── 3. completions (detect caller's login shell) ──────────────────────────────
# `$SUDO_USER` tells us who invoked sudo; fall back to "root" otherwise.
SHELL_BIN="$(getent passwd "${SUDO_USER:-root}" 2>/dev/null | cut -d: -f7)"
SHELL_BASE="$(basename "${SHELL_BIN:-/bin/sh}")"
case "$SHELL_BASE" in
    bash)
        DEST="/etc/bash_completion.d/qvm"
        "$BIN" completions bash > "$DEST" 2>/dev/null
        ;;
    zsh)
        DEST="/usr/share/zsh/site-functions/_qvm"
        mkdir -p "$(dirname "$DEST")"
        "$BIN" completions zsh > "$DEST" 2>/dev/null
        ;;
    fish)
        DEST="/usr/share/fish/completions/qvm.fish"
        mkdir -p "$(dirname "$DEST")"
        "$BIN" completions fish > "$DEST" 2>/dev/null
        ;;
    *)
        DEST=""
        ;;
esac
[ -n "${DEST:-}" ] && ok "$SHELL_BASE completions → $DEST"

# ── 4. source-reload for the current shell ────────────────────────────────────
# This script almost always runs piped to `sudo sh`, which is a fresh
# non-interactive sh — there's no parent shell to reload. The completions
# we just wrote will load automatically when the user opens a new terminal.
# We print the reload hint so they can pick it up in their existing one.
if [ -n "${DEST:-}" ]; then
    printf '  reload now: \033[36msource %s\033[0m  (or open a new terminal)\n' "$DEST"
fi

# ── done ──────────────────────────────────────────────────────────────────────
ok "ready — run \`sudo qvm\` to start"
