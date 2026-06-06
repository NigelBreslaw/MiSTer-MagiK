#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${1:-$HERE/target/armv7-unknown-linux-gnueabihf/release-device/mister-magic-fb}"
IMAGE="${MISTER_CROSS_IMAGE:-cross-custom-rust:armv7-unknown-linux-gnueabihf-b52a5}"

if [ ! -f "$BIN" ]; then
  echo "ERROR: binary missing: $BIN" >&2
  exit 1
fi
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"

export DOCKER_DEFAULT_PLATFORM="${DOCKER_DEFAULT_PLATFORM:-linux/amd64}"
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  echo "==> building cross helper image $IMAGE"
  docker build -t "$IMAGE" -f "$HERE/Dockerfile.cross-armv7" "$HERE"
fi

if [[ "$BIN" != "$HERE/"* ]]; then
  echo "ERROR: binary must be under $HERE so the Docker helper can read it" >&2
  exit 1
fi
REL_BIN="/project/${BIN#"$HERE/"}"

NEEDED="$(
  docker run --rm \
    --platform linux/amd64 \
    -v "$HERE:/project:ro" \
    -w /project \
    "$IMAGE" \
    bash -lc "arm-linux-gnueabihf-readelf -d '$REL_BIN' | awk '/NEEDED/ { gsub(/[][]/, \"\", \$5); print \$5 }'"
)"

echo "==> shared libraries for $BIN"
if [ -n "$NEEDED" ]; then
  echo "$NEEDED" | sed 's/^/    /'
else
  echo "    none"
fi

if echo "$NEEDED" | grep -E '^libav(codec|format|util|filter|device)|^libsw(resample|scale)' >/dev/null; then
  echo "ERROR: FFmpeg is dynamically linked; video builds must use static project-local FFmpeg." >&2
  exit 1
fi
