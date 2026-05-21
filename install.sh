#!/bin/sh
# install.sh — download the latest qvm static binary, install it, and set up shell completions.
# Usage: curl -fsSL https://raw.githubusercontent.com/Harsh-2002/qvm/main/install.sh | sh
set -eu

REPO="Harsh-2002/qvm"
BINARY="qvm"
INSTALL_DIR="/usr/local/bin"

# ---------- helpers -----------------------------------------------------------

die() { echo "error: $*" >&2; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' is required but not found in PATH."
}

# ---------- checks ------------------------------------------------------------

need curl
need install

# Must run as root (the binary goes to /usr/local/bin and we set up system-wide completions).
if [ "$(id -u)" -ne 0 ]; then
    die "please run as root: sudo sh install.sh"
fi

# ---------- detect platform ---------------------------------------------------

OS="$(uname -s)"
ARCH="$(uname -m)"

if [ "$OS" != "Linux" ]; then
    die "unsupported OS: $OS (only Linux is supported)"
fi

if [ "$ARCH" != "x86_64" ]; then
    die "unsupported architecture: $ARCH (only x86_64 is supported)"
fi

# ---------- fetch latest release artifact from CI ----------------------------

echo "==> Fetching latest qvm artifact from GitHub..."

# Get the latest successful run's artifact download URL.
ARTIFACT_URL="$(
    curl -fsSL \
        -H "Accept: application/vnd.github+json" \
        "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null \
    | grep '"browser_download_url"' \
    | grep 'qvm-linux-amd64-static' \
    | sed 's/.*"browser_download_url": *"\([^"]*\)".*/\1/' \
    | head -n1
)"

# Fallback: pull from the latest workflow artifact (no GitHub release needed).
if [ -z "$ARTIFACT_URL" ]; then
    echo "    (no GitHub release found — downloading from latest CI run artifact)"
    ARTIFACT_URL="https://nightly.link/${REPO}/workflows/build/main/qvm-linux-amd64-static.zip"
    DOWNLOAD_ZIP=1
else
    DOWNLOAD_ZIP=0
fi

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

if [ "${DOWNLOAD_ZIP:-0}" = "1" ]; then
    need unzip
    echo "==> Downloading ${ARTIFACT_URL} ..."
    curl -fsSL --progress-bar -o "$TMPDIR/qvm.zip" "$ARTIFACT_URL"
    unzip -q "$TMPDIR/qvm.zip" -d "$TMPDIR"
    BIN_PATH="$TMPDIR/qvm"
else
    echo "==> Downloading ${ARTIFACT_URL} ..."
    curl -fsSL --progress-bar -o "$TMPDIR/qvm" "$ARTIFACT_URL"
    BIN_PATH="$TMPDIR/qvm"
fi

chmod +x "$BIN_PATH"

# Sanity-check the downloaded binary.
"$BIN_PATH" --version >/dev/null 2>&1 || die "downloaded binary failed to execute — unexpected format or wrong architecture."

# ---------- install binary ----------------------------------------------------

echo "==> Installing ${BINARY} to ${INSTALL_DIR}/${BINARY}"
install -m 0755 "$BIN_PATH" "${INSTALL_DIR}/${BINARY}"

echo "==> $(${INSTALL_DIR}/${BINARY} --version)"

# ---------- shell completions -------------------------------------------------

SHELL_NAME="$(basename "${SHELL:-/bin/sh}")"

install_completion_bash() {
    COMP_DIR="/etc/bash_completion.d"
    if [ -d "$COMP_DIR" ]; then
        echo "==> Installing bash completion -> ${COMP_DIR}/qvm"
        "${INSTALL_DIR}/${BINARY}" completions bash > "${COMP_DIR}/qvm"
    else
        echo "    (bash completion dir ${COMP_DIR} not found — skipping)"
    fi
}

install_completion_zsh() {
    ZSH_SITE="/usr/share/zsh/site-functions"
    if [ -d "$ZSH_SITE" ]; then
        echo "==> Installing zsh completion -> ${ZSH_SITE}/_qvm"
        "${INSTALL_DIR}/${BINARY}" completions zsh > "${ZSH_SITE}/_qvm"
    else
        echo "    (zsh site-functions dir ${ZSH_SITE} not found — skipping)"
    fi
}

install_completion_fish() {
    FISH_DIR="/usr/share/fish/completions"
    if [ -d "$FISH_DIR" ]; then
        echo "==> Installing fish completion -> ${FISH_DIR}/qvm.fish"
        "${INSTALL_DIR}/${BINARY}" completions fish > "${FISH_DIR}/qvm.fish"
    else
        echo "    (fish completions dir ${FISH_DIR} not found — skipping)"
    fi
}

# Install completion for the current shell, then also bash as a baseline.
case "$SHELL_NAME" in
    bash)  install_completion_bash ;;
    zsh)   install_completion_zsh; install_completion_bash ;;
    fish)  install_completion_fish ;;
    *)     install_completion_bash ;;
esac

# ---------- done --------------------------------------------------------------

echo ""
echo "qvm installed successfully."
echo ""
echo "Next steps:"
echo "  sudo qvm doctor           # check host dependencies"
echo "  sudo qvm doctor --install # install missing dependencies"
echo "  sudo qvm init --pull-all  # first-run setup + download base images"
echo ""
echo "Docs: https://github.com/${REPO}"
