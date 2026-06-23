#!/usr/bin/env bash
# Attach locally-built macOS assets to an existing GitHub release + CDN, merging
# their checksums into the release's checksums.txt.
#
# macOS runners on GitHub Actions cost ~10x Linux, so the release pipeline skips
# the mac build (BUILD_MACOS_IN_CI unset) and the arm64 mac assets are built and
# published locally with `make release-macos && make publish-macos TAG=vX.Y.Z`.
#
# Required env: TAG (vX.Y.Z), RELEASE_VERSION (X.Y.Z — no leading v).
# Optional CDN: DO_SPACES_ACCESS_KEY + DO_SPACES_SECRET_KEY (or AWS_ACCESS_KEY_ID
#               + AWS_SECRET_ACCESS_KEY). Without them, GitHub upload still runs
#               and the CDN/updater step is skipped with a notice.
set -euo pipefail

REPO="NeboLoop/nebo"
TAG="${TAG:?set TAG=vX.Y.Z}"
VER="${RELEASE_VERSION:?set RELEASE_VERSION=X.Y.Z}"
BIN="nebo-darwin-arm64"
DMG="Nebo-${VER}-arm64.dmg"

cd "$(dirname "$0")/.."
[ -f "dist/${BIN}" ] || { echo "missing dist/${BIN} — run 'make release-macos' first"; exit 1; }
[ -f "dist/${DMG}" ] || { echo "missing dist/${DMG} — run 'make release-macos' first"; exit 1; }

echo "==> Uploading macOS assets to GitHub release ${TAG}"
gh release upload "${TAG}" "dist/${BIN}" "dist/${DMG}" --clobber --repo "${REPO}"

echo "==> Merging mac checksums into the release checksums.txt"
work="$(mktemp -d)"; trap 'rm -rf "$work"' EXIT
# CI's release job writes checksums.txt for linux/windows; pull it if present,
# strip any stale mac lines, then append the freshly-built mac sums.
if gh release download "${TAG}" --repo "${REPO}" -p checksums.txt -D "$work" 2>/dev/null; then
  grep -vE " (${BIN}|${DMG})$" "$work/checksums.txt" > "$work/merged.txt" || true
else
  : > "$work/merged.txt"
fi
cat dist/checksums-macos.txt >> "$work/merged.txt"
sort -u "$work/merged.txt" -o "$work/checksums.txt"
gh release upload "${TAG}" "$work/checksums.txt" --clobber --repo "${REPO}"
echo "checksums.txt now:"; sed 's/^/    /' "$work/checksums.txt"

# ── CDN (best-effort) — the auto-updater reads these ──────────────────────────
AKEY="${DO_SPACES_ACCESS_KEY:-${AWS_ACCESS_KEY_ID:-}}"
SKEY="${DO_SPACES_SECRET_KEY:-${AWS_SECRET_ACCESS_KEY:-}}"
if [ -n "$AKEY" ] && [ -n "$SKEY" ] && command -v aws >/dev/null 2>&1; then
  echo "==> Uploading mac assets + merged checksums + version.json to CDN (DO Spaces)"
  export AWS_ACCESS_KEY_ID="$AKEY" AWS_SECRET_ACCESS_KEY="$SKEY"
  # Use the explicit keys, not an ambient named profile (e.g. AWS_PROFILE=hp-dev).
  unset AWS_PROFILE AWS_DEFAULT_PROFILE
  EP="https://nyc3.digitaloceanspaces.com"

  # version.json — the auto-updater's "latest" pointer. Generated from the tag so
  # it is always correct/idempotent regardless of whether CI's release job also
  # wrote it. Same schema the CI release job emits.
  cat > "$work/version.json" <<EOFJ
{
  "version": "${TAG}",
  "release_url": "https://github.com/${REPO}/releases/tag/${TAG}",
  "published_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOFJ

  for f in "dist/${BIN}" "dist/${DMG}" "$work/checksums.txt"; do
    aws s3 cp "$f" "s3://neboloop/releases/${TAG}/$(basename "$f")" \
      --endpoint-url "$EP" --acl public-read
  done
  # Per-tag snapshot + the global latest pointer.
  aws s3 cp "$work/version.json" "s3://neboloop/releases/${TAG}/version.json" \
    --endpoint-url "$EP" --acl public-read --content-type application/json
  aws s3 cp "$work/version.json" "s3://neboloop/releases/version.json" \
    --endpoint-url "$EP" --acl public-read --content-type application/json
  echo "CDN updated: releases/${TAG}/ (${BIN}, ${DMG}, checksums.txt, version.json) + latest pointer"
else
  echo "==> Skipping CDN upload (no DO Spaces creds in env, or aws CLI missing)."
  echo "    NOTE: CI's release job already writes version.json + the latest pointer"
  echo "    to the CDN. This step only adds the mac assets + merged checksums."
  echo "    The GitHub release now has the mac assets; to feed the auto-updater on mac,"
  echo "    set DO_SPACES_ACCESS_KEY + DO_SPACES_SECRET_KEY and re-run, or upload"
  echo "    dist/${BIN}, dist/${DMG}, and the merged checksums.txt to"
  echo "    s3://neboloop/releases/${TAG}/."
fi
echo "Done."
