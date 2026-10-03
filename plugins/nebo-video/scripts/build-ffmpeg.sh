#!/usr/bin/env bash
# Builds the static ffmpeg + ffprobe that nebo-video embeds.
#
#   scripts/build-ffmpeg.sh <rust-target-triple>
#
# Output: vendor/<triple>/ffmpeg[.exe] and ffprobe[.exe]. build.rs refuses to
# compile the plugin without them. Every source is pinned by version and
# sha256 (x264 by commit), so a given triple always builds the same binaries.
#
# Build prerequisites: a C/C++ toolchain, make, nasm, pkg-config, meson, ninja,
# curl, git. Windows targets are cross-compiled on Linux with mingw-w64
# (x86_64-w64-mingw32-gcc/g++); the result is a standalone .exe, so the same
# build serves both the -gnu and -msvc Rust targets.
set -euo pipefail

FFMPEG_VERSION=8.1.3
FFMPEG_SHA256=7138d28c96d9d3e3af4ee3d8cad72741f8ffb40da90c1112235dea3ecd3178a3
FREETYPE_VERSION=2.14.3
FREETYPE_SHA256=36bc4f1cc413335368ee656c42afca65c5a3987e8768cc28cf11ba775e785a5f
HARFBUZZ_VERSION=13.0.0
HARFBUZZ_SHA256=1626ebc763d28f4bcca1531fef42e92ca995d45f8ad90ad2ae0b5d1a567fe67a
ZLIB_VERSION=1.3.2
ZLIB_SHA256=d7a0654783a4da529d1bb793b7ad9c3318020af77667bcae35f95d0e42a792f3
X264_COMMIT=b35605ace3ddf7c1a5d67a2eb553f034aef41d55   # x264 "stable", 2025-06-08

TARGET="${1:?usage: $0 <rust-target-triple>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$ROOT/build/$TARGET"
PREFIX="$WORK/prefix"
OUT="$ROOT/vendor/$TARGET"
SRC="$ROOT/build/src"
JOBS="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"

EXE=""
CROSS_PREFIX=""
CC=gcc
CXX=g++
HOST=""            # autotools --host; empty for a native Linux build
MESON_SYSTEM=""    # meson host_machine; empty for a native Linux build
MESON_CPU=""
FF_TARGET=()
CFLAGS="-O2"
LDFLAGS=""
EXTRA_LIBS=""      # C++ runtime that the static harfbuzz needs at link time

case "$TARGET" in
  x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu)
    # Fully static: the extracted executable needs nothing from the host.
    LDFLAGS="-static"
    EXTRA_LIBS="-lstdc++"
    ;;
  aarch64-apple-darwin|x86_64-apple-darwin)
    CC=clang
    CXX=clang++
    ARCH="${TARGET%%-*}"; [[ "$ARCH" == aarch64 ]] && ARCH=arm64
    CFLAGS="$CFLAGS -arch $ARCH -mmacosx-version-min=11.0"
    LDFLAGS="-arch $ARCH -mmacosx-version-min=11.0"
    EXTRA_LIBS="-lc++"
    HOST="$TARGET"
    MESON_SYSTEM=darwin
    MESON_CPU="${TARGET%%-*}"
    FF_TARGET=(--enable-cross-compile --arch="${TARGET%%-*}" --target-os=darwin)
    ;;
  x86_64-pc-windows-gnu|x86_64-pc-windows-msvc)
    EXE=".exe"
    CROSS_PREFIX="x86_64-w64-mingw32-"
    CC="${CROSS_PREFIX}gcc"
    CXX="${CROSS_PREFIX}g++"
    LDFLAGS="-static"
    EXTRA_LIBS="-lstdc++"
    HOST=x86_64-w64-mingw32
    MESON_SYSTEM=windows
    MESON_CPU=x86_64
    FF_TARGET=(--enable-cross-compile --arch=x86_64 --target-os=mingw32 --cross-prefix="$CROSS_PREFIX")
    ;;
  *)
    echo "unsupported target: $TARGET" >&2
    exit 1
    ;;
esac

HOST_FLAG=()
[[ -n "$HOST" ]] && HOST_FLAG=(--host="$HOST")
LIB_LDFLAGS="${LDFLAGS/-static/}"   # libraries are archives; only the final exe links -static

sha256_ok() { # file sha256
  if command -v sha256sum >/dev/null; then
    echo "$2  $1" | sha256sum -c --status -
  else
    echo "$2  $1" | shasum -a 256 -c -s -
  fi
}

fetch() { # url file sha256 -> extracts into $WORK
  local url="$1" file="$SRC/$2" sum="$3"
  mkdir -p "$SRC"
  if [[ ! -f "$file" ]]; then
    curl -fsSL -o "$file.part" "$url"
    mv "$file.part" "$file"
  fi
  if ! sha256_ok "$file" "$sum"; then
    echo "checksum mismatch: $file" >&2
    rm -f "$file"
    exit 1
  fi
  tar -xf "$file" -C "$WORK"
}

step() { # name dir command...
  local name="$1" dir="$2" rc; shift 2
  echo "==> $name"
  # Not `if ! (...)`: errexit is ignored inside a condition, so a failing
  # command mid-build would be skipped instead of stopping the step.
  set +e
  (set -e; cd "$dir"; "$@") >"$WORK/$name.log" 2>&1
  rc=$?
  set -e
  if [[ $rc -ne 0 ]]; then
    tail -60 "$WORK/$name.log"
    echo "$name failed; full log: $WORK/$name.log" >&2
    exit 1
  fi
}

export PKG_CONFIG_PATH="$PREFIX/lib/pkgconfig"
export PKG_CONFIG_LIBDIR="$PREFIX/lib/pkgconfig"   # never pick up host libraries

rm -rf "$WORK"
mkdir -p "$WORK" "$PREFIX" "$OUT"

# ---- zlib (png codecs) ----
fetch "https://zlib.net/zlib-$ZLIB_VERSION.tar.xz" "zlib-$ZLIB_VERSION.tar.xz" "$ZLIB_SHA256"
build_zlib() {
  CC="$CC" CFLAGS="$CFLAGS" LDFLAGS="$LIB_LDFLAGS" CHOST="$HOST" \
    ./configure --static --prefix="$PREFIX"
  make -j"$JOBS" libz.a
  make install
}
step zlib "$WORK/zlib-$ZLIB_VERSION" build_zlib

# ---- freetype (glyph rendering for drawtext) ----
fetch "https://download.savannah.gnu.org/releases/freetype/freetype-$FREETYPE_VERSION.tar.xz" \
  "freetype-$FREETYPE_VERSION.tar.xz" "$FREETYPE_SHA256"
build_freetype() {
  CC="$CC" CFLAGS="$CFLAGS" LDFLAGS="$LIB_LDFLAGS" ./configure "${HOST_FLAG[@]}" \
    --prefix="$PREFIX" --enable-static --disable-shared \
    --without-zlib --without-bzip2 --without-png --without-harfbuzz --without-brotli
  make -j"$JOBS"
  make install
}
step freetype "$WORK/freetype-$FREETYPE_VERSION" build_freetype

# ---- harfbuzz (text shaping; ffmpeg's drawtext requires it) ----
fetch "https://github.com/harfbuzz/harfbuzz/releases/download/$HARFBUZZ_VERSION/harfbuzz-$HARFBUZZ_VERSION.tar.xz" \
  "harfbuzz-$HARFBUZZ_VERSION.tar.xz" "$HARFBUZZ_SHA256"
MESON_CROSS=()
if [[ -n "$MESON_SYSTEM" ]]; then
  read -r -a cflags_arr <<<"$CFLAGS"
  read -r -a ldflags_arr <<<"$LIB_LDFLAGS"
  quote() { local out="" x; for x in "$@"; do out+="'$x',"; done; echo "[${out%,}]"; }
  cat >"$WORK/meson-cross.ini" <<EOF
[binaries]
c = '$CC'
cpp = '$CXX'
ar = '${CROSS_PREFIX}ar'
strip = '${CROSS_PREFIX}strip'
pkg-config = 'pkg-config'

[built-in options]
c_args = $(quote "${cflags_arr[@]}")
cpp_args = $(quote "${cflags_arr[@]}")
c_link_args = $(quote "${ldflags_arr[@]}")
cpp_link_args = $(quote "${ldflags_arr[@]}")

[host_machine]
system = '$MESON_SYSTEM'
cpu_family = '$MESON_CPU'
cpu = '$MESON_CPU'
endian = 'little'
EOF
  MESON_CROSS=(--cross-file "$WORK/meson-cross.ini")
fi
build_harfbuzz() {
  CC="$CC" CXX="$CXX" meson setup build "${MESON_CROSS[@]}" \
    --prefix="$PREFIX" --libdir=lib --buildtype=release \
    --default-library=static -Dauto_features=disabled \
    -Dfreetype=enabled -Draster=disabled -Dvector=disabled -Dsubset=disabled \
    -Dtests=disabled -Ddocs=disabled -Dutilities=disabled -Dintrospection=disabled
  ninja -C build -j"$JOBS"
  ninja -C build install
}
step harfbuzz "$WORK/harfbuzz-$HARFBUZZ_VERSION" build_harfbuzz

# ---- x264 (H.264 encoder, GPL) ----
mkdir -p "$WORK/x264"
build_x264() {
  git init -q .
  git fetch -q --depth 1 https://code.videolan.org/videolan/x264.git "$X264_COMMIT"
  git checkout -q FETCH_HEAD
  [[ "$(git rev-parse HEAD)" == "$X264_COMMIT" ]]
  local cross=()
  [[ -n "$CROSS_PREFIX" ]] && cross=(--cross-prefix="$CROSS_PREFIX")
  CC="$CC" ./configure "${HOST_FLAG[@]}" "${cross[@]}" --prefix="$PREFIX" \
    --enable-static --disable-cli --enable-pic \
    --extra-cflags="$CFLAGS" --extra-ldflags="$LIB_LDFLAGS"
  make -j"$JOBS"
  make install
}
step x264 "$WORK/x264" build_x264

# ---- ffmpeg: only what nebo-video uses ----
fetch "https://ffmpeg.org/releases/ffmpeg-$FFMPEG_VERSION.tar.xz" \
  "ffmpeg-$FFMPEG_VERSION.tar.xz" "$FFMPEG_SHA256"

join() { local IFS=,; echo "$*"; }
DEMUXERS=(mov matroska wav mp3 aac flac ogg image2 png_pipe jpeg_pipe)
DECODERS=(h264 hevc vp8 vp9 aac aac_fixed mp3 mp3float opus vorbis flac
          pcm_s16le pcm_s16be pcm_s24le pcm_s32le pcm_f32le pcm_f64le png mjpeg prores
          wrapped_avframe rawvideo)
PARSERS=(h264 hevc vp8 vp9 aac mpegaudio opus vorbis flac png mjpeg)
BSFS=(h264_mp4toannexb hevc_mp4toannexb aac_adtstoasc)
ENCODERS=(libx264 aac png mjpeg pcm_s16le)
MUXERS=(mp4 mov image2 wav null)
FILTERS=(buffer buffersink abuffer abuffersink null anull copy split asplit
         format aformat aresample setpts asetpts trim atrim scale pad fps crop
         overlay concat drawtext amix adelay volume loudnorm silencedetect
         testsrc sine)

build_ffmpeg() {
  ./configure "${FF_TARGET[@]}" \
    --cc="$CC" --cxx="$CXX" --pkg-config=pkg-config --pkg-config-flags=--static \
    --extra-cflags="$CFLAGS -I$PREFIX/include" \
    --extra-ldflags="$LDFLAGS -L$PREFIX/lib" \
    --extra-libs="$EXTRA_LIBS" \
    --enable-static --disable-shared \
    --enable-gpl --enable-libx264 --enable-libfreetype --enable-libharfbuzz --enable-zlib \
    --disable-everything --disable-autodetect --disable-network \
    --disable-doc --disable-debug --disable-ffplay \
    --enable-ffmpeg --enable-ffprobe \
    --enable-protocol=file,pipe \
    --enable-indev=lavfi \
    --enable-demuxer="$(join "${DEMUXERS[@]}")" \
    --enable-decoder="$(join "${DECODERS[@]}")" \
    --enable-parser="$(join "${PARSERS[@]}")" \
    --enable-bsf="$(join "${BSFS[@]}")" \
    --enable-encoder="$(join "${ENCODERS[@]}")" \
    --enable-muxer="$(join "${MUXERS[@]}")" \
    --enable-filter="$(join "${FILTERS[@]}")" | tee configure.out
  # configure only warns when a requested component loses a dependency;
  # a plugin that silently lacks drawtext or png is exactly what we must not ship.
  if grep -q "WARNING: Disabled" configure.out; then
    grep "WARNING: Disabled" configure.out
    return 1
  fi
  make -j"$JOBS"
}
step ffmpeg "$WORK/ffmpeg-$FFMPEG_VERSION" build_ffmpeg

FFDIR="$WORK/ffmpeg-$FFMPEG_VERSION"
for bin in ffmpeg ffprobe; do
  install -m 0755 "$FFDIR/$bin$EXE" "$OUT/$bin$EXE"
  case "$TARGET" in
    *apple-darwin)
      strip -x "$OUT/$bin"
      # CI passes the Developer ID identity; a local build gets an ad-hoc signature.
      codesign --force --options runtime --timestamp \
        --sign "${NEBO_VIDEO_CODESIGN_IDENTITY:--}" "$OUT/$bin"
      ;;
    *)
      "${CROSS_PREFIX}strip" "$OUT/$bin$EXE"
      ;;
  esac
done

ls -l "$OUT"
