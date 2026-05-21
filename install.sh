#!/bin/sh
# install.sh — install or update qvm.
#
# What this does (ONLY these things):
#   1. Download the latest qvm binary from CI and install it to /usr/local/bin
#   2. Install host dependencies (virsh, virt-install, qemu-img, genisoimage, wget)
#   3. Install shell completions for bash / zsh / fish
#   4. Print `qvm --help` and the next-step hint
#
# It does NOT configure qvm. Run `sudo qvm init` interactively after install.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Harsh-2002/qvm/main/install.sh | sudo sh
#
# Run as root (sudo). Tested on Debian, Ubuntu, Fedora, Alpine, Arch.
set -eu

REPO="Harsh-2002/qvm"
INSTALL_DIR="/usr/local/bin"
CONFIG_PATH="/etc/qvm/config.toml"
ARTIFACT_URL="https://nightly.link/${REPO}/workflows/build/main/qvm-linux-amd64-static.zip"

# ---------- helpers -----------------------------------------------------------

die()  { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
info() { printf '\033[34m==>\033[0m %s\n' "$*"; }
ok()   { printf '\033[32m  ok\033[0m %s\n' "$*"; }

need() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' is required but not found. Install it and retry."
}

# ---------- guards ------------------------------------------------------------

[ "$(uname -s)" = "Linux" ] || die "only Linux is supported."
[ "$(uname -m)" = "x86_64" ] || die "only x86_64 is supported."
[ "$(id -u)" -eq 0 ]         || die "please run as root: sudo sh install.sh"

need curl
need unzip

# Detect "update" vs "first install" before we touch anything.
if [ -f "$CONFIG_PATH" ]; then
    IS_UPDATE=1
else
    IS_UPDATE=0
fi

# ---------- 1. download and install binary ------------------------------------

info "Downloading latest qvm binary..."

TMPDIR="$(mktemp -d)"
# shellcheck disable=SC2064
trap "rm -rf '$TMPDIR'" EXIT

curl -fsSL --progress-bar -o "$TMPDIR/qvm.zip" "$ARTIFACT_URL"
unzip -q "$TMPDIR/qvm.zip" -d "$TMPDIR"

BIN="$TMPDIR/qvm"
chmod +x "$BIN"
"$BIN" --version >/dev/null 2>&1 || die "downloaded binary failed sanity check — wrong arch?"

install -m 0755 "$BIN" "${INSTALL_DIR}/qvm"
ok "qvm $("${INSTALL_DIR}/qvm" --version) installed to ${INSTALL_DIR}/qvm"

# ---------- 2. install host dependencies via qvm doctor ----------------------

info "Installing host dependencies..."
"${INSTALL_DIR}/qvm" doctor --install --yes \
    || die "failed to install host dependencies — install them manually and re-run this script."

# ---------- 3. shell completions ---------------------------------------------

info "Installing shell completions..."

# Stderr from `qvm completions <shell>` is the install-hint comment block —
# noise for an automated installer. Silence it; we only want the script itself.

BASH_COMP_DIR="/etc/bash_completion.d"
if [ -d "$BASH_COMP_DIR" ]; then
    "${INSTALL_DIR}/qvm" completions bash > "${BASH_COMP_DIR}/qvm" 2>/dev/null
    ok "bash completion -> ${BASH_COMP_DIR}/qvm"
fi

ZSH_COMP_DIR="/usr/share/zsh/site-functions"
if [ -d "$ZSH_COMP_DIR" ]; then
    "${INSTALL_DIR}/qvm" completions zsh > "${ZSH_COMP_DIR}/_qvm" 2>/dev/null
    ok "zsh completion -> ${ZSH_COMP_DIR}/_qvm"
fi

FISH_COMP_DIR="/usr/share/fish/completions"
if [ -d "$FISH_COMP_DIR" ]; then
    "${INSTALL_DIR}/qvm" completions fish > "${FISH_COMP_DIR}/qvm.fish" 2>/dev/null
    ok "fish completion -> ${FISH_COMP_DIR}/qvm.fish"
fi

# ---------- 4. final output ---------------------------------------------------

printf '\n'
printf '\033[32m================================================\033[0m\n'
if [ "$IS_UPDATE" -eq 1 ]; then
    printf '\033[32m  qvm updated. Existing config preserved.\033[0m\n'
else
    printf '\033[32m  qvm installed.\033[0m\n'
fi
printf '\033[32m================================================\033[0m\n'
printf '\n'

# Print the binary's own help — single source of truth for the command surface.
"${INSTALL_DIR}/qvm" --help
printf '\n'

printf 'Reload completions in your current shell:\n'
printf '  bash:  source /etc/bash_completion.d/qvm\n'
printf '  zsh:   source /usr/share/zsh/site-functions/_qvm\n'
printf '  fish:  source /usr/share/fish/completions/qvm.fish\n'
printf '(New terminals load completions automatically.)\n'
printf '\n'

if [ "$IS_UPDATE" -eq 0 ]; then
    printf '\033[33mNext step:\033[0m run \033[1msudo qvm init\033[0m to configure qvm interactively.\n'
    printf '(The wizard will ask about bridge, defaults, SSH keys, and optionally pull base images.)\n'
    printf '\n'
fi

printf 'Docs: https://github.com/%s\n' "$REPO"
