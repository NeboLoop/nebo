#!/usr/bin/env bash
set -euo pipefail

APP_DIR="$(cd "$(dirname "$0")/app" && pwd)"
OUT_DIR="$(cd "$(dirname "$0")" && pwd)/dist"
mkdir -p "$OUT_DIR"

# Map NeboLoop platform names to Rust targets
declare -A TARGETS=(
  [darwin-arm64]=aarch64-apple-darwin
  [darwin-amd64]=x86_64-apple-darwin
  [linux-arm64]=aarch64-unknown-linux-gnu
  [linux-amd64]=x86_64-unknown-linux-gnu
  [windows-amd64]=x86_64-pc-windows-gnu
)

NATIVE_TARGETS=(darwin-arm64 darwin-amd64)
CROSS_TARGETS=(linux-arm64 linux-amd64 windows-amd64)
BINARY_NAME="notepad"

echo "=== Building notepad for all platforms ==="

# --- Native macOS builds (sequential, same target dir) ---
for platform in "${NATIVE_TARGETS[@]}"; do
  target="${TARGETS[$platform]}"
  echo ""
  echo "--- $platform ($target) via cargo ---"
  rustup target add "$target" 2>/dev/null || true
  cargo build --manifest-path "$APP_DIR/Cargo.toml" --release --target "$target"
  cp "$APP_DIR/target/$target/release/$BINARY_NAME" "$OUT_DIR/$BINARY_NAME-$platform"
  echo "  -> $OUT_DIR/$BINARY_NAME-$platform"
done

# --- Cross builds (concurrent, each in own Docker container) ---
# Override sccache since it doesn't exist in cross containers
export CROSS_CONFIG="$APP_DIR/Cross.toml"

PIDS=()
PLATFORMS_RUNNING=()

for platform in "${CROSS_TARGETS[@]}"; do
  target="${TARGETS[$platform]}"
  echo ""
  echo "--- $platform ($target) via cross (background) ---"

  (
    RUSTC_WRAPPER="" cross build --manifest-path "$APP_DIR/Cargo.toml" --release --target "$target" 2>&1
    ext=""
    [[ "$platform" == windows-* ]] && ext=".exe"
    cp "$APP_DIR/target/$target/release/${BINARY_NAME}${ext}" "$OUT_DIR/$BINARY_NAME-$platform${ext}"
    echo "  -> $OUT_DIR/$BINARY_NAME-$platform${ext}"
  ) &

  PIDS+=($!)
  PLATFORMS_RUNNING+=("$platform")
done

echo ""
echo "=== Waiting for ${#PIDS[@]} cross builds... ==="

FAILED=0
for i in "${!PIDS[@]}"; do
  pid="${PIDS[$i]}"
  platform="${PLATFORMS_RUNNING[$i]}"
  if wait "$pid"; then
    echo "  [OK] $platform"
  else
    echo "  [FAIL] $platform"
    FAILED=$((FAILED + 1))
  fi
done

echo ""
if [ "$FAILED" -gt 0 ]; then
  echo "=== $FAILED build(s) failed ==="
  exit 1
else
  echo "=== All builds complete ==="
fi
ls -lh "$OUT_DIR"/
