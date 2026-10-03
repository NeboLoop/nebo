#!/usr/bin/env bash
# Downloads the prebuilt static ffmpeg + ffprobe that nebo-video embeds.
#
#   scripts/fetch-ffmpeg.sh <rust-target-triple>
#
# Output: vendor/<triple>/ffmpeg.gz and ffprobe.gz (ffmpeg.exe.gz /
# ffprobe.exe.gz on Windows). build.rs embeds them and refuses to compile
# without them. Every download is pinned to ffmpeg 8.1.2 and checked against
# the sha256 its publisher lists.
#
#   Linux, macOS: Martin Riedl's GPL builds  https://ffmpeg.martin-riedl.de
#   Windows:      Gyan Doshi's "essentials"  https://www.gyan.dev/ffmpeg/builds
#
# Needs curl, unzip, gzip and sha256sum (or shasum). Runs on any host.
set -euo pipefail

TARGET="${1:?usage: $0 <rust-target-triple>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CACHE="$ROOT/build/downloads"
OUT="$ROOT/vendor/$TARGET"

MR=https://ffmpeg.martin-riedl.de/download
case "$TARGET" in
  x86_64-unknown-linux-gnu)
    FFMPEG_URL=$MR/linux/amd64/1783011670_8.1.2/ffmpeg.zip
    FFMPEG_SHA256=56452c0bfc4ee0325cd615d62f46ba8264f62eed34f727c2224c6c84fa7b8719
    FFPROBE_URL=$MR/linux/amd64/1783011670_8.1.2/ffprobe.zip
    FFPROBE_SHA256=c6f2d36e98f9a4445fad0b0be539f4c4faf13fd502116bf131becd53f56cd390
    ;;
  aarch64-unknown-linux-gnu)
    FFMPEG_URL=$MR/linux/arm64/1783010599_8.1.2/ffmpeg.zip
    FFMPEG_SHA256=ab9e16864b6bf4ae7e13bbdbdc29621be11a5c547c57af8d4250e9fa2f5e6461
    FFPROBE_URL=$MR/linux/arm64/1783010599_8.1.2/ffprobe.zip
    FFPROBE_SHA256=fb78317b81cdeb614533be59e489019b754afd199670666af28f0e9574be395b
    ;;
  x86_64-apple-darwin)
    FFMPEG_URL=$MR/macos/amd64/1783018342_8.1.2/ffmpeg.zip
    FFMPEG_SHA256=a52ef43883f44c219766d4b3bdde4e635b35465d0b704c01c3a0566b59775df9
    FFPROBE_URL=$MR/macos/amd64/1783018342_8.1.2/ffprobe.zip
    FFPROBE_SHA256=5408ca588c8c72b0dde3afe676d0a7acf25ef97e55ae6eba5c7bede1cda42695
    ;;
  aarch64-apple-darwin)
    FFMPEG_URL=$MR/macos/arm64/1783011502_8.1.2/ffmpeg.zip
    FFMPEG_SHA256=ef1aa60006c7b77ce170c1608c08d8e4ba1c30c5746f2ac986ded932d0ac2c3c
    FFPROBE_URL=$MR/macos/arm64/1783011502_8.1.2/ffprobe.zip
    FFPROBE_SHA256=c39787f4af7a3932502d2d48db6f6feaaa836b48a73ef78c32cc3285df61dfaf
    ;;
  x86_64-pc-windows-msvc|x86_64-pc-windows-gnu)
    # One archive holds both executables.
    FFMPEG_URL=https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-8.1.2-essentials_build.zip
    FFMPEG_SHA256=db580001caa24ac104c8cb856cd113a87b0a443f7bdf47d8c12b1d740584a2ec
    FFPROBE_URL=$FFMPEG_URL
    FFPROBE_SHA256=$FFMPEG_SHA256
    ;;
  *)
    echo "unsupported target: $TARGET" >&2
    exit 1
    ;;
esac

EXE=""
[[ "$TARGET" == *windows* ]] && EXE=".exe"

sha256_ok() { # file sha256
  if command -v sha256sum >/dev/null; then
    echo "$2  $1" | sha256sum -c --status -
  else
    echo "$2  $1" | shasum -a 256 -c -s -
  fi
}

download() { # url sha256 -> prints the cached path
  local url="$1" sum="$2" file
  file="$CACHE/$sum.zip"
  if [[ ! -f "$file" ]]; then
    mkdir -p "$CACHE"
    curl -fsSL -o "$file.part" "$url"
    mv "$file.part" "$file"
  fi
  if ! sha256_ok "$file" "$sum"; then
    rm -f "$file"
    echo "checksum mismatch for $url" >&2
    exit 1
  fi
  echo "$file"
}

rm -rf "$OUT"
mkdir -p "$OUT"
for bin in ffmpeg ffprobe; do
  if [[ "$bin" == ffmpeg ]]; then
    zip=$(download "$FFMPEG_URL" "$FFMPEG_SHA256")
  else
    zip=$(download "$FFPROBE_URL" "$FFPROBE_SHA256")
  fi
  member=$(unzip -Z1 "$zip" | grep -E "(^|/)$bin$EXE\$")
  unzip -p "$zip" "$member" | gzip -9 -n >"$OUT/$bin$EXE.gz"
done

ls -l "$OUT"
