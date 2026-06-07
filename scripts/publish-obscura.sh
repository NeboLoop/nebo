#!/usr/bin/env bash
# Build the Obscura tier-2 browser from our fork (localrivet/obscura) for the
# HOST platform and publish the binaries as release assets the Nebo release CI
# consumes (.github/workflows/release.yml → "Stage Obscura sidecars").
#
# Obscura is bundled CORE software (a Tauri externalBin sidecar), NOT a marketplace
# .napp. Asset names MUST be exactly "<name>-<triple>" (+ .exe on Windows) — Tauri's
# externalBin and the CI fetch both key off that name.
#
# Run this once per platform (macOS arm64, macOS x86_64, Linux x86_64, Linux arm64,
# Windows x86_64) — V8 makes obscura impractical to cross-compile, so build natively.
#
# Usage:
#   OBSCURA_TAG=cdp-compat-v1 scripts/publish-obscura.sh [/path/to/obscura/fork]
#
# Requires: gh (authenticated to localrivet), cargo, the obscura fork checkout.
set -euo pipefail

OBSCURA_REPO="${1:-$(cd "$(dirname "$0")/../.." && pwd)/obscura}"
OBSCURA_FORK="${OBSCURA_FORK:-localrivet/obscura}"
OBSCURA_TAG="${OBSCURA_TAG:-cdp-compat-v1}"

TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
EXT=""
case "$TRIPLE" in *windows*) EXT=".exe" ;; esac

echo "Fork:   $OBSCURA_REPO"
echo "Repo:   $OBSCURA_FORK"
echo "Tag:    $OBSCURA_TAG"
echo "Triple: $TRIPLE"

[ -d "$OBSCURA_REPO" ] || { echo "ERROR: obscura fork not found at $OBSCURA_REPO"; exit 1; }

echo "Building obscura-cli (release)..."
( cd "$OBSCURA_REPO" && cargo build --release -p obscura-cli )

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
for bin in obscura obscura-worker; do
  src="$OBSCURA_REPO/target/release/$bin$EXT"
  [ -f "$src" ] || { echo "ERROR: $src not produced by the build"; exit 1; }
  cp "$src" "$STAGE/$bin-$TRIPLE$EXT"
done

# Create the release if it doesn't exist yet, then upload (clobbering same-named assets).
if ! gh release view "$OBSCURA_TAG" --repo "$OBSCURA_FORK" >/dev/null 2>&1; then
  echo "Creating release $OBSCURA_TAG on $OBSCURA_FORK..."
  gh release create "$OBSCURA_TAG" --repo "$OBSCURA_FORK" \
    --title "Obscura for Nebo ($OBSCURA_TAG)" \
    --notes "CDP-compat fork binaries consumed by Nebo's release CI as externalBin sidecars."
fi

echo "Uploading assets for $TRIPLE..."
gh release upload "$OBSCURA_TAG" --repo "$OBSCURA_FORK" --clobber \
  "$STAGE/obscura-$TRIPLE$EXT" "$STAGE/obscura-worker-$TRIPLE$EXT"

echo "Done. Published:"
echo "  obscura-$TRIPLE$EXT"
echo "  obscura-worker-$TRIPLE$EXT"
