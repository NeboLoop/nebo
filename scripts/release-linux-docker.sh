#!/usr/bin/env bash
# Local Linux release build (amd64) — the anti-Actions-outage path.
# Produces the same artifacts as CI's build-linux into dist/:
#   nebo-linux-amd64, nebo-linux-amd64-headless, nebo_<ver>_amd64.deb,
#   Nebo-<ver>-amd64.AppImage
#
# arm64 is CI-only for now: it needs 12G swap for the LTO link even on native
# hardware; under emulation it is not practical.
set -euo pipefail
cd "$(dirname "$0")/.."

IMG=nebo-linux-builder:amd64
OBSCURA_FORK="https://github.com/localrivet/obscura"
OBSCURA_REF="chromiumoxide-cdp-compat"

echo "==> Frontend (host)"
[ -d app/build ] || (cd app && pnpm install --frozen-lockfile && pnpm build)

echo "==> Builder image (cached after first run)"
docker build --platform linux/amd64 -t "$IMG" -f docker/linux-release.Dockerfile docker/

echo "==> Linux build (target dir target-linux-amd64/, kept for incremental rebuilds)"
mkdir -p dist target-linux-amd64
docker run --rm --platform linux/amd64 \
  -v "$PWD":/work \
  -e CARGO_TARGET_DIR=/work/target-linux-amd64 \
  -e APPIMAGE_EXTRACT_AND_RUN=1 -e NO_STRIP=true \
  -e OBSCURA_FORK="$OBSCURA_FORK" -e OBSCURA_REF="$OBSCURA_REF" \
  "$IMG" bash -euxc '
    MULTIARCH=$(dpkg-architecture -qDEB_HOST_MULTIARCH)
    export PKG_CONFIG_PATH=/usr/lib/${MULTIARCH}/pkgconfig
    # -lopenblas is REQUIRED (turbovec cblas_sgemm) — same flags as CI.
    export RUSTFLAGS="-C link-arg=-L/usr/lib/${MULTIARCH} -C link-arg=-Wl,--no-as-needed -C link-arg=-lopenblas -C link-arg=-Wl,--as-needed"

    # Obscura sidecars, same fork+ref as CI.
    TRIPLE="$(rustc -vV | sed -n "s/^host: //p")"
    if [ ! -x "src-tauri/binaries/obscura-${TRIPLE}" ]; then
      rm -rf /tmp/obscura-src
      git clone --depth 1 --branch "$OBSCURA_REF" "$OBSCURA_FORK" /tmp/obscura-src
      ( cd /tmp/obscura-src && CARGO_TARGET_DIR=/tmp/obscura-target cargo build --release -p obscura-cli )
      mkdir -p src-tauri/binaries
      for bin in obscura obscura-worker; do
        cp "/tmp/obscura-target/release/${bin}" "src-tauri/binaries/${bin}-${TRIPLE}"
        chmod +x "src-tauri/binaries/${bin}-${TRIPLE}"
      done
    fi

    cargo tauri build
    cargo build --release -p nebo-cli

    PKG_VERSION=$(sed -n "s/^version = \"\(.*\)\"/\1/p" Cargo.toml | head -1)
    cp "$CARGO_TARGET_DIR/release/nebo" dist/nebo-linux-amd64
    cp "$CARGO_TARGET_DIR/release/nebo-cli" dist/nebo-linux-amd64-headless
    cp "$CARGO_TARGET_DIR"/release/bundle/deb/*.deb dist/ 2>/dev/null || true
    APPIMAGE=$(ls "$CARGO_TARGET_DIR"/release/bundle/appimage/*.AppImage 2>/dev/null | head -n1 || true)
    [ -n "$APPIMAGE" ] && cp "$APPIMAGE" "dist/Nebo-${PKG_VERSION}-amd64.AppImage"
    ls -la dist/
  '
echo "==> Done: dist/"
