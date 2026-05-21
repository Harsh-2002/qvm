#!/bin/sh
# install.sh — full qvm setup in one command.
#
# What this does:
#   1. Download and install the latest static qvm binary
#   2. Install all host dependencies (virsh, virt-install, qemu-img, genisoimage, wget)
#   3. Write /etc/qvm/config.toml and create data directories
#   4. Download all five built-in distro base images
#   5. Install shell completions (bash / zsh / fish) and print the reload command
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Harsh-2002/qvm/main/install.sh | sudo sh
#
# Run as root (sudo). Tested on Debian, Ubuntu, Fedora, Alpine, Arch.
set -eu

REPO="Harsh-2002/qvm"
INSTALL_DIR="/usr/local/bin"
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

info "Installing host dependencies (virsh, virt-install, qemu-img, genisoimage, wget)..."
# doctor --install is interactive but non-interactive when all deps present.
# Pipe yes so it auto-confirms the install prompt on first run.
printf 'yes\n' | "${INSTALL_DIR}/qvm" doctor --install || true

# ---------- 3 + 4. first-run setup + download all base images ----------------

info "Running qvm init --yes --pull-all (config, dirs, and all 5 distro images)..."
info "This may take several minutes depending on your connection."
"${INSTALL_DIR}/qvm" init --yes --pull-all
ok "Setup complete."

# ---------- 5. shell completions ---------------------------------------------

info "Installing shell completions..."

# bash
BASH_COMP_DIR="/etc/bash_completion.d"
if [ -d "$BASH_COMP_DIR" ]; then
    "${INSTALL_DIR}/qvm" completions bash > "${BASH_COMP_DIR}/qvm"
    ok "bash completion -> ${BASH_COMP_DIR}/qvm"
fi

# zsh
ZSH_COMP_DIR="/usr/share/zsh/site-functions"
if [ -d "$ZSH_COMP_DIR" ]; then
    "${INSTALL_DIR}/qvm" completions zsh > "${ZSH_COMP_DIR}/_qvm"
    ok "zsh completion -> ${ZSH_COMP_DIR}/_qvm"
fi

# fish (system-wide)
FISH_COMP_DIR="/usr/share/fish/completions"
if [ -d "$FISH_COMP_DIR" ]; then
    "${INSTALL_DIR}/qvm" completions fish > "${FISH_COMP_DIR}/qvm.fish"
    ok "fish completion -> ${FISH_COMP_DIR}/qvm.fish"
fi

# ---------- done --------------------------------------------------------------

printf '\n'
printf '\033[32m================================================\033[0m\n'
printf '\033[32m  qvm is ready.\033[0m\n'
printf '\033[32m================================================\033[0m\n'
printf '\n'
printf 'Reload completions in your current shell:\n'
printf '\n'
printf '  bash:  source /etc/bash_completion.d/qvm\n'
printf '  zsh:   source /usr/share/zsh/site-functions/_qvm\n'
printf '  fish:  source /usr/share/fish/completions/qvm.fish\n'
printf '\n'
printf 'Or just open a new terminal — completions load automatically.\n'
printf '\n'
printf 'Quick start:\n'
printf '  qvm ls                             # list VMs\n'
printf '  qvm run myvm ubuntu:24.04          # create a VM\n'
printf '  qvm ssh-cmd myvm                   # get the ssh command\n'
printf '\n'
printf 'Docs: https://github.com/%s\n' "$REPO"
