#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${1:-$HERE/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb}"
IMAGE="${MISTER_CROSS_IMAGE:-cross-custom-rust:armv7-unknown-linux-gnueabihf-b52a5}"
APPLE_IMAGE="${MISTER_APPLE_CONTAINER_IMAGE:-mister-magik-cross-armv7:ubuntu20-arm64}"

if [ ! -f "$BIN" ]; then
  echo "ERROR: binary missing: $BIN" >&2
  exit 1
fi
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"

export DOCKER_DEFAULT_PLATFORM="${DOCKER_DEFAULT_PLATFORM:-linux/amd64}"
if command -v arm-linux-gnueabihf-readelf >/dev/null 2>&1; then
  NEEDED="$(
    arm-linux-gnueabihf-readelf -d "$BIN" \
      | awk '/NEEDED/ { gsub(/[][]/, "", $5); print $5 }'
  )"
  GLIBC_VERSIONS="$(
    arm-linux-gnueabihf-readelf --version-info "$BIN" \
      | grep -o 'GLIBC_[0-9.]*' \
      | sort -Vu
  )"
else
  if [[ "$BIN" != "$HERE/"* ]]; then
    echo "ERROR: binary must be under $HERE so the container helper can read it" >&2
    exit 1
  fi
  REL_BIN="/project/${BIN#"$HERE/"}"

  if [ "$(uname -s)" = Darwin ] && [ "$(uname -m)" = arm64 ] && command -v container >/dev/null 2>&1; then
    if ! container image inspect "$APPLE_IMAGE" >/dev/null 2>&1; then
      echo "==> building Apple-container helper image $APPLE_IMAGE"
      container build --arch arm64 --file "$HERE/Dockerfile.cross-armv7" --tag "$APPLE_IMAGE" "$HERE"
    fi
    RUNNER=(
      container run --arch arm64 --rm
      --volume "$HERE:/project:ro"
      --workdir /project
      "$APPLE_IMAGE"
    )
  else
    export DOCKER_DEFAULT_PLATFORM="${DOCKER_DEFAULT_PLATFORM:-linux/amd64}"
    if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
      echo "==> building cross helper image $IMAGE"
      docker build -t "$IMAGE" -f "$HERE/Dockerfile.cross-armv7" "$HERE"
    fi
    RUNNER=(
      docker run --rm
      --platform linux/amd64
      -v "$HERE:/project:ro"
      -w /project
      "$IMAGE"
    )
  fi

  NEEDED="$(
    "${RUNNER[@]}" \
      bash -lc "arm-linux-gnueabihf-readelf -d '$REL_BIN' | awk '/NEEDED/ { gsub(/[][]/, \"\", \$5); print \$5 }'"
  )"
  GLIBC_VERSIONS="$(
    "${RUNNER[@]}" \
      bash -lc "arm-linux-gnueabihf-readelf --version-info '$REL_BIN' | grep -o 'GLIBC_[0-9.]*' | sort -Vu"
  )"
fi

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

MAX_GLIBC="$(
  {
    printf '%s\n' "$GLIBC_VERSIONS"
    printf '%s\n' GLIBC_2.31
  } | sed '/^$/d' | sort -Vu | tail -1
)"
if [ -n "$MAX_GLIBC" ]; then
  echo "==> max GLIBC symbol version: $MAX_GLIBC"
fi
if [ "$MAX_GLIBC" != GLIBC_2.31 ]; then
  echo "ERROR: binary requires $MAX_GLIBC, but MiSTer glibc is 2.31." >&2
  exit 1
fi
