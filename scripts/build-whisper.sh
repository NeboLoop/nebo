#!/usr/bin/env bash
# Build whisper.cpp static library for the current platform.
# Called by `make desktop` if vendor/whisper/lib/libwhisper.a is missing.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
VENDOR_DIR="$PROJECT_ROOT/third_party/whisper"
BUILD_DIR="/tmp/whisper-cpp-build"

# Target output
LIB_DIR="$VENDOR_DIR/lib"
INCLUDE_DIR="$VENDOR_DIR/include"

if [ -f "$LIB_DIR/libwhisper.a" ] && [ -f "$INCLUDE_DIR/whisper.h" ]; then
    echo "[build-whisper] Static library already exists at $LIB_DIR/libwhisper.a"
    exit 0
fi

echo "[build-whisper] Building whisper.cpp static library..."

# Clone whisper.cpp if not already present
if [ ! -d "$BUILD_DIR" ]; then
    git clone --depth 1 https://github.com/ggerganov/whisper.cpp.git "$BUILD_DIR"
else
    echo "[build-whisper] Using existing clone at $BUILD_DIR"
fi

cd "$BUILD_DIR"

# Detect CPU count
NCPU=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)

# Platform-specific cmake flags
CMAKE_EXTRA=""
case "$(uname -s)" in
    Darwin)
        CMAKE_EXTRA="-DGGML_METAL=ON -DGGML_METAL_EMBED_LIBRARY=ON"
        ;;
    *)
        CMAKE_EXTRA="-DGGML_METAL=OFF"
        ;;
esac

# Build static library
cmake -B build \
    -DCMAKE_BUILD_TYPE=Release \
    -DBUILD_SHARED_LIBS=OFF \
    -DWHISPER_BUILD_EXAMPLES=OFF \
    -DWHISPER_BUILD_TESTS=OFF \
    $CMAKE_EXTRA

cmake --build build -j"$NCPU" --config Release

# Install to vendor directory
mkdir -p "$LIB_DIR" "$INCLUDE_DIR"

# Library ends up in build/src/ on recent whisper.cpp
if [ -f "build/src/libwhisper.a" ]; then
    cp build/src/libwhisper.a "$LIB_DIR/"
elif [ -f "build/libwhisper.a" ]; then
    cp build/libwhisper.a "$LIB_DIR/"
else
    echo "[build-whisper] Searching for libwhisper.a..."
    find build -name "libwhisper.a" -exec cp {} "$LIB_DIR/" \;
fi

# Also copy ggml lib if present (needed for linking)
find build -name "libggml*.a" -exec cp {} "$LIB_DIR/" \; 2>/dev/null || true

# Copy headers (whisper + all ggml headers it includes)
if [ -d "include" ]; then
    cp include/*.h "$INCLUDE_DIR/" 2>/dev/null || true
fi
if [ -d "ggml/include" ]; then
    cp ggml/include/*.h "$INCLUDE_DIR/" 2>/dev/null || true
fi

# Verify
if [ ! -f "$LIB_DIR/libwhisper.a" ]; then
    echo "[build-whisper] ERROR: libwhisper.a not found after build"
    exit 1
fi

echo "[build-whisper] Static library installed to $LIB_DIR/libwhisper.a"
echo "[build-whisper] Header installed to $INCLUDE_DIR/whisper.h"
ls -la "$LIB_DIR/"
