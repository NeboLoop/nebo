#!/bin/sh
# Runs one cargo command for the server image (Dockerfile, build stage).
#
# Link flags: `-l openblas` is REQUIRED — turbovec's `cblas_sgemm` reference
# otherwise goes unresolved at link time (blas-src is a link-only shim the
# linker drops). `--no-as-needed` forces the lib to stay on the link line
# regardless of placement; flip back to `--as-needed` so unrelated libs still
# get pruned. Same flags as .github/workflows/release.yml (tested on arm64 +
# amd64).
#
# Compile cache: when the build is handed a cache bucket and its keys (CI passes
# them as BuildKit secrets, so they never land in a layer), every rustc and C/C++
# compile goes through sccache backed by that bucket. A workspace crate whose
# inputs did not change since any earlier build comes back from the bucket
# instead of being compiled again. Without the keys (a local `docker build`)
# cargo runs exactly as before. A cache that cannot start never fails the build.
set -eu

MULTIARCH=$(dpkg-architecture -qDEB_HOST_MULTIARCH)
export RUSTFLAGS="-C link-arg=-L/usr/lib/${MULTIARCH} -C link-arg=-Wl,--no-as-needed -C link-arg=-lopenblas -C link-arg=-Wl,--as-needed"

cache=""
if [ -n "${SCCACHE_BUCKET:-}" ] && [ -n "${AWS_ACCESS_KEY_ID:-}" ] && [ -n "${AWS_SECRET_ACCESS_KEY:-}" ]; then
  # Everything this image compiles lives under one prefix of the bucket.
  export SCCACHE_S3_KEY_PREFIX=nebo-server/
  if sccache --start-server; then
    cache=1
    # cc-rs picks sccache up from RUSTC_WRAPPER; the cmake crate (whisper.cpp)
    # reads the launcher variables.
    export RUSTC_WRAPPER=sccache CMAKE_C_COMPILER_LAUNCHER=sccache CMAKE_CXX_COMPILER_LAUNCHER=sccache
  else
    echo "compile cache unavailable; building without it"
  fi
fi

"$@"

if [ -n "$cache" ]; then
  # Stopping the server flushes the uploads still in flight before the layer
  # is committed, and prints the hit/miss counts.
  sccache --show-stats
  sccache --stop-server >/dev/null
fi
