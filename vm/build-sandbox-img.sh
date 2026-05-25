#!/bin/bash
# Build the nebo-vm sandbox image containing the VM daemon + settings.
#
# This is the small, frequently-updated image that gets bundled with
# every Nebo release. The rootfs (Alpine + runtimes) is separate and
# rarely changes.
#
# Usage:
#   ./vm/build-sandbox-img.sh [arch]
#   arch: arm64 (default) or x64
#
# Output:
#   vm/build/nebo-vm.{arch}.img

set -euo pipefail

ARCH="${1:-arm64}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
BUILD_DIR="$SCRIPT_DIR/build"
IMG_SIZE_MB=16

mkdir -p "$BUILD_DIR"

# Map arch to Rust target
case "$ARCH" in
    arm64|aarch64)
        RUST_TARGET="aarch64-unknown-linux-musl"
        IMG_NAME="nebo-vm.arm64.img"
        ;;
    x64|x86_64|amd64)
        RUST_TARGET="x86_64-unknown-linux-musl"
        IMG_NAME="nebo-vm.x64.img"
        ;;
    *)
        echo "ERROR: unknown arch: $ARCH (expected arm64 or x64)"
        exit 1
        ;;
esac

echo "=== Building nebo-vm-daemon for $RUST_TARGET ==="

# Cross-compile the VM daemon as a static musl binary
cd "$PROJECT_DIR"
cargo build --target "$RUST_TARGET" --release -p nebo-vm-daemon

DAEMON_BINARY="target/$RUST_TARGET/release/nebo-vm-daemon"
if [ ! -f "$DAEMON_BINARY" ]; then
    echo "ERROR: daemon binary not found at $DAEMON_BINARY"
    exit 1
fi

BINARY_SIZE=$(stat -f%z "$DAEMON_BINARY" 2>/dev/null || stat -c%s "$DAEMON_BINARY")
echo "Daemon binary size: $(( BINARY_SIZE / 1024 / 1024 ))MB"

echo "=== Creating nebo-vm image ==="

# Create an empty image file
IMG_PATH="$BUILD_DIR/$IMG_NAME"
dd if=/dev/zero of="$IMG_PATH" bs=1M count=$IMG_SIZE_MB 2>/dev/null

# Format as FAT32 (simple, works everywhere)
if command -v newfs_msdos &>/dev/null; then
    newfs_msdos -F 32 "$IMG_PATH"
elif command -v mkfs.vfat &>/dev/null; then
    mkfs.vfat -F 32 "$IMG_PATH"
else
    echo "ERROR: no FAT32 formatter found (need newfs_msdos or mkfs.vfat)"
    exit 1
fi

# Mount and copy files in
MOUNT_DIR=$(mktemp -d)
trap "rm -rf $MOUNT_DIR" EXIT

if [[ "$(uname)" == "Darwin" ]]; then
    DEVICE=$(hdiutil attach -nomount "$IMG_PATH" | head -1 | awk '{print $1}')
    mount -t msdos "$DEVICE" "$MOUNT_DIR"

    cp "$DAEMON_BINARY" "$MOUNT_DIR/nebo-vm-daemon"
    cp "$SCRIPT_DIR/settings.json" "$MOUNT_DIR/settings.json" 2>/dev/null || true

    umount "$MOUNT_DIR"
    hdiutil detach "$DEVICE" -quiet
else
    sudo mount -o loop "$IMG_PATH" "$MOUNT_DIR"

    sudo cp "$DAEMON_BINARY" "$MOUNT_DIR/nebo-vm-daemon"
    sudo cp "$SCRIPT_DIR/settings.json" "$MOUNT_DIR/settings.json" 2>/dev/null || true

    sudo umount "$MOUNT_DIR"
fi

FINAL_SIZE=$(stat -f%z "$IMG_PATH" 2>/dev/null || stat -c%s "$IMG_PATH")
echo "=== Done: $IMG_PATH ($(( FINAL_SIZE / 1024 / 1024 ))MB) ==="
echo "Contents:"
echo "  nebo-vm-daemon  $(( BINARY_SIZE / 1024 ))KB"
echo "  settings.json   (network allowlist, proxy config)"
