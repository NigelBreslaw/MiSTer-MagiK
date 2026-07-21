#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
apple_container_cpus() { getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.logicalcpu; }
apple_container_memory() { printf '8g\n'; }
ROOT="$(cd "$HERE/.." && pwd)"
BIN="${1:-$HERE/target/armv7-unknown-linux-gnueabihf/release-device-profile/mister-magik-fb}"
OUT_DIR="${2:-$ROOT/build/binary-size-analysis}"
IMAGE="${MISTER_CROSS_IMAGE:-$(python3 "$ROOT/scripts/checks/ci-cache-identity.py" --value cross_image)}"
APPLE_IMAGE="${MISTER_APPLE_CONTAINER_IMAGE:-mister-magik-cross-armv7:ubuntu20-arm64}"

if [ ! -f "$BIN" ]; then
  echo "ERROR: binary missing: $BIN" >&2
  echo "Hint: build an unstripped diagnostic binary first: scripts/agent build runtime-profile" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
RAW="$OUT_DIR/nm-symbols.tsv"
GROUPS_FILE="$OUT_DIR/groups.tsv"
TOP="$OUT_DIR/top-symbols.tsv"

if [[ "$BIN" != "$HERE/"* ]]; then
  echo "ERROR: binary must be under $HERE so the container helper can read it" >&2
  exit 1
fi
REL_BIN="/project/${BIN#"$HERE/"}"

if [ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ] && command -v container >/dev/null 2>&1; then
  echo "==> using Apple-container helper image $APPLE_IMAGE"
  CONTAINER_CPUS="$(apple_container_cpus)"
  CONTAINER_MEMORY="$(apple_container_memory)"
  container build --arch arm64 --file "$HERE/Dockerfile.cross-armv7" --tag "$APPLE_IMAGE" "$HERE" >/dev/null
  container run --arch arm64 --rm \
    --cpus "$CONTAINER_CPUS" \
    --memory "$CONTAINER_MEMORY" \
    --volume "$HERE:/project" \
    --workdir /project \
    "$APPLE_IMAGE" \
    bash -lc "arm-linux-gnueabihf-nm -S --size-sort --radix=d '$REL_BIN' 2>/dev/null" \
    >"$RAW.tmp" || true
else
  export DOCKER_DEFAULT_PLATFORM="${DOCKER_DEFAULT_PLATFORM:-linux/amd64}"
  if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "==> building cross helper image $IMAGE"
    docker build -t "$IMAGE" -f "$HERE/Dockerfile.cross-armv7" "$HERE"
  fi
  docker run --rm \
    --platform linux/amd64 \
    --user "$(id -u):$(id -g)" \
    -v "$HERE:/project" \
    -w /project \
    "$IMAGE" \
    bash -lc "arm-linux-gnueabihf-nm -S --size-sort --radix=d '$REL_BIN' 2>/dev/null" \
    >"$RAW.tmp" || true
fi

if [ ! -s "$RAW.tmp" ]; then
  rm -f "$RAW.tmp"
  echo "ERROR: no symbols found. Use an unstripped build such as --profile." >&2
  exit 1
fi

awk '
  NF >= 4 {
    name = $4
    for (i = 5; i <= NF; i++) name = name " " $i
    print $1 "\t" $2 "\t" $3 "\t" name
  }
' "$RAW.tmp" >"$RAW"
rm -f "$RAW.tmp"

awk -F '\t' '
  function group(name) {
    if (name ~ /ffmpeg|avcodec|avformat|avutil|swscale|h264|mov|mpeg|cabac|golomb/) return "FFmpeg/video"
    if (name ~ /slint|i_slint|corelib|software_renderer|femtovg/) return "Slint/generated UI"
    if (name ~ /swash|font|glyph|ttf|typeface|textlayout/) return "Fonts/text"
    if (name ~ /sqlite|rusqlite|catalog|library|quick_xml|walkdir/) return "SQLite/catalog"
    if (name ~ /png|zune|preview|image/) return "PNG/preview"
    if (name ~ /launcher|input|gamepad|joystick|evdev|fpga|fb|vt/) return "Launcher/input/fb"
    return "Other"
  }
  {
    size = $2 + 0
    g = group($4)
    bytes[g] += size
    count[g] += 1
    total += size
  }
  END {
    print "group\tbytes\tpercent\tsymbols"
    for (g in bytes) {
      pct = total ? (bytes[g] * 100.0 / total) : 0
      printf "%s\t%d\t%.2f\t%d\n", g, bytes[g], pct, count[g]
    }
  }
' "$RAW" >"$GROUPS_FILE.tmp"
{
  head -1 "$GROUPS_FILE.tmp"
  tail -n +2 "$GROUPS_FILE.tmp" | sort -t "$(printf '\t')" -k2,2nr
} >"$GROUPS_FILE"
rm -f "$GROUPS_FILE.tmp"

{
  echo "bytes\ttype\tsymbol"
  sort -t "$(printf '\t')" -k2,2nr "$RAW" | awk -F '\t' 'NR <= 100 { print $2 "\t" $3 "\t" $4 }'
} >"$TOP"

echo "==> size analysis"
echo "    symbols: $RAW"
echo "    groups:  $GROUPS_FILE"
echo "    top:     $TOP"
column -t -s "$(printf '\t')" "$GROUPS_FILE" 2>/dev/null || cat "$GROUPS_FILE"
