#!/usr/bin/env bash
#
# Build, publish, and bundle Nebo plugins as signed .napp files.
#
# Usage:
#   ./scripts/publish-plugins.sh build          # Build all plugins for current platform
#   ./scripts/publish-plugins.sh build gws      # Build a single plugin
#   ./scripts/publish-plugins.sh publish         # Publish all to NeboAI
#   ./scripts/publish-plugins.sh publish gws     # Publish a single plugin
#   ./scripts/publish-plugins.sh bundle          # Copy built binaries into bundled-napps/
#
# Prerequisites:
#   - Rust toolchain with cross-compilation targets
#   - Plugin repos at $REPOS_DIR (default: ../repos/plugins)
#   - NeboAI CLI or API access for publishing
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPOS_DIR="${REPOS_DIR:-$(cd "$PROJECT_DIR/../repos/plugins" 2>/dev/null && pwd || echo "")}"
BUNDLED_DIR="$PROJECT_DIR/src-tauri/bundled-napps/plugins"

# ── Plugin Registry ──────────────────────────────────────────────────────────
# slug:cli_crate:platforms (comma-separated)
# Platforms: d-a64=darwin-arm64, d-x64=darwin-amd64, l-a64=linux-arm64, l-x64=linux-amd64, w-x64=windows-amd64

MUST_BUNDLE=(
    "gws:google-workspace-cli:all"
    "digest:digest-cli:all"
    "nebo-pdf:nebo-pdf:all"
    "nebo-office:cli:all"
    "email:email-cli:all"
    "peek:peek-cli:darwin"
    "imessage:imessage-cli:darwin"
    "reminders:reminders-cli:all"
    "watchdog:watchdog-cli:all"
)

SHOULD_BUNDLE=(
    "social:social-cli:all"
    "devlink:devlink-cli:all"
    "imagegen:imagegen-cli:all"
    "ffmpeg:ffmpeg-cli:all"
    "slack:slack-cli:all"
)

ALL_PLUGINS=("${MUST_BUNDLE[@]}" "${SHOULD_BUNDLE[@]}")

# ── Platform Detection ───────────────────────────────────────────────────────

detect_platform() {
    local os arch
    os="$(uname -s | tr '[:upper:]' '[:lower:]')"
    arch="$(uname -m)"

    case "$os" in
        darwin) os="darwin" ;;
        linux)  os="linux" ;;
        *)      os="windows" ;;
    esac

    case "$arch" in
        x86_64|amd64)  arch="amd64" ;;
        aarch64|arm64) arch="arm64" ;;
    esac

    echo "${os}-${arch}"
}

CURRENT_PLATFORM="$(detect_platform)"

# Rust target triples
platform_to_target() {
    case "$1" in
        darwin-arm64)  echo "aarch64-apple-darwin" ;;
        darwin-amd64)  echo "x86_64-apple-darwin" ;;
        linux-arm64)   echo "aarch64-unknown-linux-gnu" ;;
        linux-amd64)   echo "x86_64-unknown-linux-gnu" ;;
        windows-amd64) echo "x86_64-pc-windows-msvc" ;;
    esac
}

# Expand platform shorthand
expand_platforms() {
    case "$1" in
        all)    echo "darwin-arm64 darwin-amd64 linux-arm64 linux-amd64 windows-amd64" ;;
        darwin) echo "darwin-arm64 darwin-amd64" ;;
        linux)  echo "linux-arm64 linux-amd64" ;;
        *)      echo "$1" ;;
    esac
}

# ── Build ────────────────────────────────────────────────────────────────────

build_plugin() {
    local slug="$1" cli_crate="$2" platform_spec="$3"
    local src_dir="$REPOS_DIR/$slug"

    if [ ! -d "$src_dir" ]; then
        echo "  SKIP $slug (repo not found at $src_dir)"
        return 1
    fi

    local platforms
    platforms="$(expand_platforms "$platform_spec")"

    for platform in $platforms; do
        # Only build for current platform unless cross-compilation is set up
        if [ "$platform" != "$CURRENT_PLATFORM" ]; then
            echo "  SKIP $slug/$platform (cross-compile: use CI)"
            continue
        fi

        local target
        target="$(platform_to_target "$platform")"

        echo "  BUILD $slug for $platform ($target)..."
        (cd "$src_dir" && cargo build --release -p "$cli_crate" ${target:+--target "$target"} 2>&1) || {
            echo "  FAIL $slug/$platform"
            return 1
        }

        # Find the binary
        local binary_name="$slug"
        if [ -f "$src_dir/plugin.json" ]; then
            binary_name=$(python3 -c "
import json
p = json.load(open('$src_dir/plugin.json'))
bn = p.get('platforms',{}).get('$platform',{}).get('binaryName','$slug')
print(bn)
" 2>/dev/null || echo "$slug")
        fi

        local binary_path="$src_dir/target/$target/release/$binary_name"
        if [ ! -f "$binary_path" ]; then
            # Try without target triple
            binary_path="$src_dir/target/release/$binary_name"
        fi

        if [ -f "$binary_path" ]; then
            local size
            size=$(stat -f%z "$binary_path" 2>/dev/null || stat -c%s "$binary_path" 2>/dev/null || echo "?")
            echo "  OK   $slug/$platform ($size bytes)"
        else
            echo "  FAIL $slug/$platform (binary not found)"
            return 1
        fi
    done
}

cmd_build() {
    local filter="${1:-}"

    if [ -z "$REPOS_DIR" ] || [ ! -d "$REPOS_DIR" ]; then
        echo "ERROR: Plugin repos not found. Set REPOS_DIR to the plugins directory."
        echo "  Expected: $REPOS_DIR"
        exit 1
    fi

    echo "Building plugins for $CURRENT_PLATFORM..."
    echo ""

    local built=0 skipped=0 failed=0

    for entry in "${ALL_PLUGINS[@]}"; do
        IFS=: read -r slug cli_crate platform_spec <<< "$entry"

        if [ -n "$filter" ] && [ "$slug" != "$filter" ]; then
            continue
        fi

        if build_plugin "$slug" "$cli_crate" "$platform_spec"; then
            ((built++))
        else
            ((failed++))
        fi
    done

    echo ""
    echo "Built: $built  Failed: $failed"
}

# ── Publish ──────────────────────────────────────────────────────────────────

cmd_publish() {
    local filter="${1:-}"

    echo "Publishing plugins to NeboAI..."
    echo ""
    echo "NOTE: This requires NeboAI API access. For each plugin:"
    echo "  1. Create/update the plugin artifact on NeboAI"
    echo "  2. Upload the binary for each platform"
    echo "  3. Submit for review (auto-approved for first-party)"
    echo ""

    for entry in "${ALL_PLUGINS[@]}"; do
        IFS=: read -r slug cli_crate platform_spec <<< "$entry"

        if [ -n "$filter" ] && [ "$slug" != "$filter" ]; then
            continue
        fi

        local src_dir="$REPOS_DIR/$slug"
        if [ ! -f "$src_dir/plugin.json" ]; then
            echo "  SKIP $slug (no plugin.json)"
            continue
        fi

        local version
        version=$(python3 -c "import json; print(json.load(open('$src_dir/plugin.json'))['version'])" 2>/dev/null || echo "0.1.0")

        echo "  PUBLISH $slug v$version"
        echo "    Plugin dir: $src_dir"
        echo "    Manifest:   $src_dir/plugin.json"
        echo "    PLUGIN.md:  $src_dir/PLUGIN.md"

        local platforms
        platforms="$(expand_platforms "$platform_spec")"
        for platform in $platforms; do
            local binary_name
            binary_name=$(python3 -c "
import json
p = json.load(open('$src_dir/plugin.json'))
bn = p.get('platforms',{}).get('$platform',{}).get('binaryName','$slug')
print(bn)
" 2>/dev/null || echo "$slug")
            local binary_path="$src_dir/target/release/$binary_name"
            if [ -f "$binary_path" ]; then
                echo "    Binary [$platform]: $binary_path"
            else
                echo "    Binary [$platform]: NOT BUILT"
            fi
        done
        echo ""
    done

    echo "To publish, use the NeboAI MCP tools or the web dashboard."
    echo "After publishing and approval, NeboAI signs each plugin and produces .napp files."
}

# ── Bundle ───────────────────────────────────────────────────────────────────

cmd_bundle() {
    echo "Copying built plugin binaries into bundled-napps/..."
    echo ""

    mkdir -p "$BUNDLED_DIR"

    local count=0
    for entry in "${ALL_PLUGINS[@]}"; do
        IFS=: read -r slug cli_crate platform_spec <<< "$entry"

        local src_dir="$REPOS_DIR/$slug"
        if [ ! -d "$src_dir" ]; then
            continue
        fi

        # Check for .napp file (signed by NeboAI)
        local napp_file="$src_dir/$slug.napp"
        if [ -f "$napp_file" ]; then
            cp "$napp_file" "$BUNDLED_DIR/$slug.napp"
            echo "  OK   $slug.napp (signed)"
            ((count++))
        else
            echo "  SKIP $slug (no .napp file — publish first)"
        fi
    done

    echo ""
    echo "Bundled: $count plugins"
    echo "Location: $BUNDLED_DIR"
    ls -la "$BUNDLED_DIR"/*.napp 2>/dev/null || echo "(none)"
}

# ── Status ───────────────────────────────────────────────────────────────────

cmd_status() {
    echo "Plugin Status"
    echo "============="
    echo ""
    printf "%-15s %-8s %-12s %-8s %-8s\n" "SLUG" "VERSION" "PLATFORMS" "BUILT?" "BUNDLED?"

    for entry in "${ALL_PLUGINS[@]}"; do
        IFS=: read -r slug cli_crate platform_spec <<< "$entry"

        local src_dir="$REPOS_DIR/$slug"
        local version="?"
        local built="no"
        local bundled="no"

        if [ -f "$src_dir/plugin.json" ]; then
            version=$(python3 -c "import json; print(json.load(open('$src_dir/plugin.json'))['version'])" 2>/dev/null || echo "?")
        fi

        # Check if binary exists for current platform
        if [ -d "$src_dir/target/release" ]; then
            local binary_name="$slug"
            if [ -f "$src_dir/plugin.json" ]; then
                binary_name=$(python3 -c "
import json
p = json.load(open('$src_dir/plugin.json'))
bn = p.get('platforms',{}).get('$CURRENT_PLATFORM',{}).get('binaryName','$slug')
print(bn)
" 2>/dev/null || echo "$slug")
            fi
            if [ -f "$src_dir/target/release/$binary_name" ]; then
                built="yes"
            fi
        fi

        if [ -f "$BUNDLED_DIR/$slug.napp" ]; then
            bundled="yes"
        fi

        printf "%-15s %-8s %-12s %-8s %-8s\n" "$slug" "$version" "$platform_spec" "$built" "$bundled"
    done
}

# ── Main ─────────────────────────────────────────────────────────────────────

case "${1:-status}" in
    build)   cmd_build "${2:-}" ;;
    publish) cmd_publish "${2:-}" ;;
    bundle)  cmd_bundle ;;
    status)  cmd_status ;;
    *)
        echo "Usage: $0 {build|publish|bundle|status} [plugin-slug]"
        exit 1
        ;;
esac
