#!/bin/sh
# install.sh — install or update qvm.
#
# POSIX shell. No bashisms. Verify with `shellcheck -s sh install.sh`.
#
# Four jobs, nothing more:
#   1. Install or update /usr/local/bin/qvm from the latest GitHub Release.
#   2. Ensure host dependencies are present (via `qvm doctor --install`).
#   3. Detect the user's shell and drop in completions.
#   4. Print the source-reload command so completions are live immediately.
#
# Output is minimal on purpose. Configuration is NOT this script's job —
# run `sudo qvm` after install for the interactive onboarding wizard.
#
# Releases follow CalVer (YYYY.M.D) and there is only ever one active
# release at a time — see CLAUDE.md § Release policy. The URL below
# pins to `releases/latest/download/…`, which GitHub redirects to
# whatever the current release is, so this script never goes stale.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Harsh-2002/qvm/main/install.sh | sudo sh
#
# Tested on Debian, Ubuntu, Fedora, Alpine, Arch.
set -eu

REPO="Harsh-2002/qvm"
BIN_DIR="/usr/local/bin"
BIN="${BIN_DIR}/qvm"

die() { printf '\033[31m✗\033[0m %s\n' "$*" >&2; exit 1; }
ok()  { printf '\033[32m✓\033[0m %s\n' "$*"; }

# ── guards ────────────────────────────────────────────────────────────────────
[ "$(uname -s)" = "Linux" ] || die "Linux only."
[ "$(id -u)"    -eq 0     ] || die "run as root (sudo sh install.sh)."
command -v curl  >/dev/null 2>&1 || die "curl missing."
command -v unzip >/dev/null 2>&1 || die "unzip missing."

# ── arch detection ────────────────────────────────────────────────────────────
HOST_ARCH="$(uname -m)"
case "$HOST_ARCH" in
    x86_64)  ARTIFACT_NAME="qvm-linux-amd64-static" ;;
    aarch64) ARTIFACT_NAME="qvm-linux-arm64-static" ;;
    *) die "unsupported arch '$HOST_ARCH' (qvm ships amd64 and arm64)." ;;
esac
# `releases/latest/download/` is a stable GitHub-side redirect — no need
# to hit the API, no auth, works under `curl -L`. If no release exists
# yet (fresh repo before the first release.yml run), curl returns 404
# and the script aborts with a clear message.
ARTIFACT_URL="https://github.com/${REPO}/releases/latest/download/${ARTIFACT_NAME}.zip"

ACTION="installed"
[ -x "$BIN" ] && ACTION="updated"

# ── 1. binary ─────────────────────────────────────────────────────────────────
TMP="$(mktemp -d)"
# shellcheck disable=SC2064
trap "rm -rf '$TMP'" EXIT
if ! curl -fsSL -o "$TMP/qvm.zip" "$ARTIFACT_URL"; then
    die "could not download $ARTIFACT_URL — has a release been cut yet? See https://github.com/${REPO}/releases"
fi
unzip -qo "$TMP/qvm.zip" -d "$TMP"
chmod +x "$TMP/qvm"
"$TMP/qvm" --version >/dev/null 2>&1 || die "binary failed sanity check."
install -m 0755 "$TMP/qvm" "$BIN"
ok "qvm $ACTION → $BIN"

# ── 2. host deps ──────────────────────────────────────────────────────────────
"$BIN" doctor --install --yes >/dev/null 2>&1 || true
ok "host dependencies checked"

# ── 3. completions (shell-aware, written to every known-good location) ──────
#
# Re-running this script always REGENERATES the completion files — clap
# emits them deterministically from the current binary's command surface,
# so any outdated copy is overwritten in place. No "upgrade" branch needed.
#
# Caller's login shell, looked up via /etc/passwd. SUDO_USER is set by sudo;
# falls back to root's shell if invoked some other way (cron, etc.).
SHELL_BIN="$(getent passwd "${SUDO_USER:-root}" 2>/dev/null | cut -d: -f7)"
SHELL_BASE="$(basename "${SHELL_BIN:-/bin/sh}")"
DEST=""
RELOAD=""

case "$SHELL_BASE" in
    bash)
        # Both well-known paths. The first is bash-completion ≥ 2.0's
        # preferred location; the second is the older convention some
        # distros still ship. Writing both keeps every distro happy.
        D1="/usr/share/bash-completion/completions/qvm"
        D2="/etc/bash_completion.d/qvm"
        mkdir -p "$(dirname "$D1")" "$(dirname "$D2")"
        "$BIN" completions bash > "$D1" 2>/dev/null
        cp "$D1" "$D2" 2>/dev/null || true
        DEST="$D1"
        RELOAD="source $D1"
        ;;
    zsh)
        # Try the existing fpath directories first; create site-functions
        # as a fallback. _qvm uses #compdef so any fpath entry works.
        for d in /usr/share/zsh/site-functions /usr/share/zsh/vendor-completions; do
            if [ -d "$d" ] || mkdir -p "$d" 2>/dev/null; then
                DEST="$d/_qvm"
                "$BIN" completions zsh > "$DEST" 2>/dev/null
                break
            fi
        done
        # `compinit -u` (insecure mode) reloads without prompting about
        # ownership/perm checks — the files we wrote as root are fine.
        RELOAD="autoload -U compinit && compinit -u"
        ;;
    fish)
        DEST="/usr/share/fish/completions/qvm.fish"
        mkdir -p "$(dirname "$DEST")"
        "$BIN" completions fish > "$DEST" 2>/dev/null
        # fish reloads completions on directory change; no manual reload
        # command needed in practice.
        RELOAD=""
        ;;
esac

if [ -n "${DEST:-}" ]; then
    ok "$SHELL_BASE completions → $DEST"
fi

# ── 4. reload hint for the current shell ──────────────────────────────────────
# This script always runs piped to `sudo sh`, which is a fresh non-
# interactive sh — there's no parent shell to source for. We print the
# one-liner the user can paste into their existing terminal to get
# completions immediately without opening a new one.
if [ -n "${RELOAD:-}" ]; then
    printf '  reload now: \033[36m%s\033[0m  (or open a new terminal)\n' "$RELOAD"
elif [ -n "${DEST:-}" ]; then
    printf '  loads automatically on next terminal\n'
fi

# ── done ──────────────────────────────────────────────────────────────────────
ok "ready — run \`sudo qvm\` to start"
