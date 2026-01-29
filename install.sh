#!/bin/bash
set -e

# GoBot installer
# Usage: curl -fsSL https://gobot.ai/install.sh | sh
#
# Environment variables:
#   GOBOT_VERSION      - Version to install (default: latest)
#   GOBOT_INSTALL_DIR  - Binary install location (default: /usr/local/bin)
#   GOBOT_DATA_DIR     - Data directory (default: ~/.gobot)

VERSION="${GOBOT_VERSION:-latest}"
INSTALL_DIR="${GOBOT_INSTALL_DIR:-/usr/local/bin}"
DATA_DIR="${GOBOT_DATA_DIR:-$HOME/.gobot}"
GITHUB_REPO="localrivet/gobot"  # TODO: Update when published

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info() { echo -e "${GREEN}==>${NC} $1"; }
warn() { echo -e "${YELLOW}Warning:${NC} $1"; }
error() { echo -e "${RED}Error:${NC} $1"; exit 1; }

# Detect OS and architecture
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "$ARCH" in
    x86_64)  ARCH="amd64" ;;
    aarch64) ARCH="arm64" ;;
    arm64)   ARCH="arm64" ;;
    *)       error "Unsupported architecture: $ARCH" ;;
esac

case "$OS" in
    darwin) OS="darwin" ;;
    linux)  OS="linux" ;;
    *)      error "Unsupported OS: $OS" ;;
esac

info "Installing GoBot for $OS/$ARCH..."

# Create temp directory
TMP_DIR=$(mktemp -d)
trap "rm -rf $TMP_DIR" EXIT

# Download binary
if [ "$VERSION" = "latest" ]; then
    DOWNLOAD_URL="https://github.com/$GITHUB_REPO/releases/latest/download/gobot-$OS-$ARCH"
else
    DOWNLOAD_URL="https://github.com/$GITHUB_REPO/releases/download/$VERSION/gobot-$OS-$ARCH"
fi

info "Downloading from $DOWNLOAD_URL..."
if ! curl -fsSL "$DOWNLOAD_URL" -o "$TMP_DIR/gobot"; then
    error "Failed to download GoBot. Check your internet connection and try again."
fi
chmod +x "$TMP_DIR/gobot"

# Install binary
if [ -w "$INSTALL_DIR" ]; then
    mv "$TMP_DIR/gobot" "$INSTALL_DIR/gobot"
else
    info "Installing to $INSTALL_DIR (requires sudo)..."
    sudo mv "$TMP_DIR/gobot" "$INSTALL_DIR/gobot"
fi

# Verify installation
if ! command -v gobot &> /dev/null; then
    # Check if it's in the install dir but not in PATH
    if [ -f "$INSTALL_DIR/gobot" ]; then
        warn "$INSTALL_DIR is not in your PATH. Add it with:"
        echo "  export PATH=\"\$PATH:$INSTALL_DIR\""
    else
        error "Installation failed. Please check permissions and try again."
    fi
fi

# The binary handles data directory creation and default file copying
# via internal/defaults package on first run

info "GoBot installed successfully!"
echo ""
echo "Get started:"
echo "  gobot              # Start GoBot (server + agent + UI)"
echo "  open http://localhost:27895"
echo ""
echo "First time setup:"
echo "  1. Open http://localhost:27895/setup"
echo "  2. Create admin account"
echo "  3. Add API keys in Settings > Providers"
echo ""
echo "Data directory: $DATA_DIR (created on first run)"
echo ""
