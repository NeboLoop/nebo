#!/usr/bin/env bash
# The bare macOS release asset (`nebo-darwin-<arch>`): the headless nebo-cli,
# signed on its own. Never the app's executable copied out of Nebo.app: that
# signature is bound to the bundle (identifier, Info.plist hash, entitlements),
# so run on its own the kernel kills it. The direct-mode updater and bare
# installs download this asset.
#
#   scripts/nebo-cli-darwin.sh sign <built nebo-cli> <out>
#       Developer ID, hardened runtime, --identifier dev.neboai.nebo.cli,
#       assets/macos/nebo-cli.entitlements. SIGN_IDENTITY unset: copied with the
#       linker's ad-hoc signature (a build without the certificate).
#   scripts/nebo-cli-darwin.sh notarize <file>
#       Zipped and submitted (a bare Mach-O can't be stapled; Gatekeeper checks
#       it online on first run). Fails unless Apple answers Accepted. Credentials:
#       APPLE_ID + APPLE_APP_PASSWORD + APPLE_TEAM_ID, else the legacy-keychain
#       item `apple-notarization` (as `make notarize`).
#   scripts/nebo-cli-darwin.sh check <file>
#       The release gate: runs `<file> --version` from a clean folder with a
#       bare environment; fails when it is killed or says anything but nebo.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
verb=${1:-}; file=${2:-}
[ -n "$verb" ] && [ -n "$file" ] || { sed -n '2,21p' "$0"; exit 2; }

case "$verb" in
sign)
  out=${3:?sign needs <built nebo-cli> <out>}
  cp "$file" "$out"
  chmod +x "$out"
  if [ -n "${SIGN_IDENTITY:-}" ]; then
    codesign --force --sign "$SIGN_IDENTITY" \
      --identifier dev.neboai.nebo.cli \
      --entitlements "$ROOT/assets/macos/nebo-cli.entitlements" \
      --timestamp --options runtime "$out"
    codesign --verify --strict --verbose=2 "$out"
    echo "signed: $out ($(codesign -dv "$out" 2>&1 | grep -E '^Identifier=' ))"
  else
    echo "SIGN_IDENTITY unset: $out keeps the linker's ad-hoc signature"
  fi
  ;;
notarize)
  work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
  zip_path="$work/$(basename "$file").zip"
  ditto -c -k --keepParent "$file" "$zip_path"
  if [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_APP_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
    creds=(--apple-id "$APPLE_ID" --password "$APPLE_APP_PASSWORD" --team-id "$APPLE_TEAM_ID")
  else
    apple_id=${NOTARIZE_APPLE_ID:-alma.tuck@gmail.com}
    pw=$(security find-generic-password -w -s "${NOTARIZE_KEYCHAIN_SERVICE:-apple-notarization}" -a "$apple_id" 2>/dev/null || true)
    [ -n "$pw" ] || { echo "no notarization credentials (APPLE_ID/APPLE_APP_PASSWORD/APPLE_TEAM_ID or the keychain item)"; exit 1; }
    creds=(--apple-id "$apple_id" --password "$pw" --team-id "${NOTARIZE_TEAM_ID:-7Y2D3KQ2UM}")
  fi
  set +e
  result=$(xcrun notarytool submit "$zip_path" "${creds[@]}" --wait 2>&1)
  set -e
  echo "$result"
  echo "$result" | grep -q "status: Accepted" || { echo "notarization of $file did NOT succeed"; exit 1; }
  ;;
check)
  # Never run the app's executable: with no role argument it opens the app.
  ident=$(codesign -dv "$file" 2>&1 | sed -n 's/^Identifier=//p')
  if [ "$ident" = dev.neboai.nebo ]; then
    echo "FAIL: $file is the app's executable (identifier dev.neboai.nebo), not nebo-cli"
    exit 1
  fi
  work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
  cp "$file" "$work/"
  bin="$work/$(basename "$file")"
  if file "$bin" | grep -q x86_64 && [ "$(uname -m)" = arm64 ] && ! arch -x86_64 /usr/bin/true 2>/dev/null; then
    echo "SKIP: $file is x86_64 and this Mac has no Rosetta"
    exit 0
  fi
  set +e
  out=$(cd "$work" && env -i HOME="$work" PATH=/usr/bin:/bin "$bin" --version 2>&1)
  code=$?
  set -e
  if [ $code -ne 0 ] || ! echo "$out" | grep -q '^nebo'; then
    echo "FAIL: $file run on its own exited $code: $out"
    [ $code -eq 137 ] && echo "(killed: its signature does not hold outside a bundle)"
    exit 1
  fi
  echo "ok: $file runs on its own: $out"
  ;;
*)
  sed -n '2,21p' "$0"; exit 2
  ;;
esac
