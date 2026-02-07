#!/bin/bash
set -e

# Nebo installer
# Usage: curl -fsSL https://raw.githubusercontent.com/nebolabs/nebo/main/install.sh | sh
#
# Environment variables:
#   NEBO_VERSION      - Version to install (default: latest)
#   NEBO_INSTALL_DIR  - Binary install location (default: /usr/local/bin)

VERSION="${NEBO_VERSION:-latest}"
INSTALL_DIR="${NEBO_INSTALL_DIR:-/usr/local/bin}"
GITHUB_REPO="nebolabs/nebo"

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

info "Installing Nebo for $OS/$ARCH..."

# Create temp directory
TMP_DIR=$(mktemp -d)
trap "rm -rf $TMP_DIR" EXIT

# On Linux, detect if desktop libraries are available; fall back to headless
SUFFIX=""
if [ "$OS" = "linux" ]; then
    if ldconfig -p 2>/dev/null | grep -q libwebkit2gtk-4.1 && ldconfig -p 2>/dev/null | grep -q libgtk-3; then
        info "Desktop libraries detected — installing desktop build."
    else
        SUFFIX="-headless"
        info "Desktop libraries not found — installing headless build."
        info "For desktop mode, install: libwebkit2gtk-4.1-0 libgtk-3-0"
    fi
fi

# Download binary
BINARY_NAME="nebo-$OS-$ARCH$SUFFIX"
if [ "$VERSION" = "latest" ]; then
    DOWNLOAD_URL="https://github.com/$GITHUB_REPO/releases/latest/download/$BINARY_NAME"
else
    DOWNLOAD_URL="https://github.com/$GITHUB_REPO/releases/download/$VERSION/$BINARY_NAME"
fi

info "Downloading from $DOWNLOAD_URL..."
if ! curl -fsSL "$DOWNLOAD_URL" -o "$TMP_DIR/nebo"; then
    error "Failed to download Nebo. Check your internet connection and try again."
fi
chmod +x "$TMP_DIR/nebo"

# Install binary
if [ -w "$INSTALL_DIR" ]; then
    mv "$TMP_DIR/nebo" "$INSTALL_DIR/nebo"
else
    info "Installing to $INSTALL_DIR (requires sudo)..."
    sudo mv "$TMP_DIR/nebo" "$INSTALL_DIR/nebo"
fi

# Verify installation
if ! command -v nebo &> /dev/null; then
    # Check if it's in the install dir but not in PATH
    if [ -f "$INSTALL_DIR/nebo" ]; then
        warn "$INSTALL_DIR is not in your PATH. Add it with:"
        echo "  export PATH=\"\$PATH:$INSTALL_DIR\""
    else
        error "Installation failed. Please check permissions and try again."
    fi
fi

# The binary handles data directory creation and default file copying
# via internal/defaults package on first run

info "Nebo installed successfully!"
echo ""
echo "Get started:"
echo "  nebo              # Start Nebo (server + agent + UI)"
echo "  open http://localhost:27895"
echo ""
echo "First time setup:"
echo "  1. Open http://localhost:27895/setup"
echo "  2. Create admin account"
echo "  3. Add API keys in Settings > Providers"
echo ""
