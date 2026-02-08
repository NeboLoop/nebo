#!/bin/bash
# create-dmg.sh — Build a macOS .dmg installer with drag-to-Applications layout.
#
# Prerequisites:
#   brew install create-dmg
#
# Usage:
#   ./scripts/create-dmg.sh [version] [arch]
#
# Examples:
#   ./scripts/create-dmg.sh              # defaults: "dev", current arch
#   ./scripts/create-dmg.sh v1.2.3 arm64
#
# Expects dist/Nebo.app to already exist (run `make app-bundle` first).

set -euo pipefail

VERSION="${1:-dev}"
ARCH="${2:-$(uname -m)}"

# Normalize arch names
case "$ARCH" in
  x86_64) ARCH="amd64" ;;
  aarch64) ARCH="arm64" ;;
esac

# Strip leading 'v' for display
PKG_VERSION="${VERSION#v}"

APP_BUNDLE="dist/Nebo.app"
DMG_NAME="Nebo-${PKG_VERSION}-${ARCH}.dmg"
DMG_PATH="dist/${DMG_NAME}"
VOLUME_NAME="Nebo ${PKG_VERSION}"

if [ ! -d "$APP_BUNDLE" ]; then
  echo "Error: ${APP_BUNDLE} not found. Run 'make app-bundle' first."
  exit 1
fi

# Remove previous DMG if it exists
rm -f "$DMG_PATH"

echo "Creating ${DMG_PATH}..."

# Check if create-dmg is available (preferred — gives the nicest result)
if command -v create-dmg >/dev/null 2>&1; then
  # create-dmg exits 2 when no background image is set (cosmetic warning).
  # The DMG is still created successfully, so we treat exit code 2 as OK.
  set +e
  create-dmg \
    --volname "$VOLUME_NAME" \
    --volicon "assets/icons/nebo.icns" \
    --window-pos 200 120 \
    --window-size 660 400 \
    --icon-size 100 \
    --icon "Nebo.app" 180 190 \
    --hide-extension "Nebo.app" \
    --app-drop-link 480 190 \
    --no-internet-enable \
    "$DMG_PATH" \
    "$APP_BUNDLE"
  EXIT_CODE=$?
  set -e
  if [ "$EXIT_CODE" -ne 0 ] && [ "$EXIT_CODE" -ne 2 ]; then
    echo "create-dmg failed with exit code $EXIT_CODE"
    exit "$EXIT_CODE"
  fi
else
  echo "create-dmg not found, falling back to hdiutil..."

  # Create a temporary directory for DMG contents
  STAGING=$(mktemp -d)
  trap 'rm -rf "$STAGING"' EXIT

  cp -R "$APP_BUNDLE" "$STAGING/"
  ln -s /Applications "$STAGING/Applications"

  hdiutil create \
    -volname "$VOLUME_NAME" \
    -srcfolder "$STAGING" \
    -ov \
    -format UDZO \
    "$DMG_PATH"
fi

echo ""
echo "Done: ${DMG_PATH}"
ls -lh "$DMG_PATH"
