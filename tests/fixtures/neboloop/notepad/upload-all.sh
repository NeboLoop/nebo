#!/usr/bin/env bash
set -euo pipefail

# NeboLoop tool ID for Notepad
TOOL_ID="a381ac8e-0b8c-4ab1-8631-2d15fb91c672"
API_BASE="https://neboloop.com/api/v1"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
DIST_DIR="$SCRIPT_DIR/dist"
TOOL_MD="$SCRIPT_DIR/TOOL.md"

if [ ! -f "$TOOL_MD" ]; then
  echo "ERROR: TOOL.md not found at $TOOL_MD"
  exit 1
fi

PLATFORMS=(darwin-arm64 darwin-amd64 linux-arm64 linux-amd64 windows-arm64 windows-amd64)

echo "=== Uploading notepad binaries to NeboLoop ==="

for platform in "${PLATFORMS[@]}"; do
  ext=""
  [[ "$platform" == windows-* ]] && ext=".exe"
  binary="$DIST_DIR/notepad-${platform}${ext}"

  if [ ! -f "$binary" ]; then
    echo "SKIP: $binary not found"
    continue
  fi

  echo ""
  echo "--- Uploading $platform ---"
  echo "  Binary: $binary ($(du -h "$binary" | cut -f1))"

  # Get upload token via NeboLoop MCP (must be done interactively)
  # For scripted use, you'd call the API directly with your auth token:
  echo "  Use NeboLoop MCP to get a token, then run:"
  echo "  curl -X POST $API_BASE/developer/apps/$TOOL_ID/binaries \\"
  echo "    -H \"Authorization: Bearer <TOKEN>\" \\"
  echo "    -F \"file=@$binary\" \\"
  echo "    -F \"platform=$platform\" \\"
  echo "    -F \"skill=@$TOOL_MD\""
done

echo ""
echo "=== Done ==="
