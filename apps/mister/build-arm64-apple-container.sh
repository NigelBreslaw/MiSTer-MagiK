#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Native Apple-container path for ARMv7 MiSTer builds on Apple Silicon.
#
# This is the GitHub macos-26 / local Apple Silicon build path. It intentionally
# does not use cross-rs or Docker's linux/amd64 compatibility path.
set -euo pipefail
cd "$(dirname "$0")"
. "$PWD/scripts/apple-container-resources.sh"

phase_now_ms() { perl -MTime::HiRes=time -e 'printf "%.0f", time * 1000'; }
phase_start() { printf 'WORKFLOW_PHASE start group=build phase=%s time_ms=%s\n' "$1" "$(phase_now_ms)"; }
phase_end() { printf 'WORKFLOW_PHASE end group=build phase=%s time_ms=%s\n' "$1" "$(phase_now_ms)"; }

REPO_ROOT="$(cd "$PWD/../.." && pwd)"
IMAGE="${MISTER_APPLE_CONTAINER_IMAGE:-mister-magik-cross-armv7:ubuntu20-arm64}"
TARGET_DIR="${MISTER_APPLE_CONTAINER_TARGET_DIR:-/private/tmp/mister-magik-apple-container-target}"
MIRROR_TARGET_DIR="${MISTER_APPLE_CONTAINER_MIRROR_TARGET_DIR:-$PWD/target}"
CARGO_CACHE="${MISTER_APPLE_CONTAINER_CARGO_HOME:-$HOME/.cargo}"
RUST_TOOLCHAIN="${MISTER_ARM64_RUST_TOOLCHAIN:-$HOME/.rustup/toolchains/stable-aarch64-unknown-linux-gnu}"
CONTAINER_MEMORY="$(apple_container_memory)"
TARGET=armv7-unknown-linux-gnueabihf
DOCKERFILE=Dockerfile.cross-armv7
IMAGE_STAMP="${MISTER_APPLE_CONTAINER_IMAGE_STAMP:-$TARGET_DIR.image.sha256}"

PROFILE=release-device
COMMAND=build
LIB_ONLY=0
FEATURES=(ui)
FEATURE_LIST=""
UI_SCOPE="${MISTER_UI_BUILD_SCOPE:-}"
UI_SCOPE_EXPLICIT=0
[ -n "$UI_SCOPE" ] && UI_SCOPE_EXPLICIT=1
LOCKED=1
VERBOSE=0
CLEAN=0
REBUILD_IMAGE="${MISTER_APPLE_CONTAINER_REBUILD_IMAGE:-0}"
BIN_TARGET=""
BIN_NAME="mister-magik-fb"
MANIFEST_PATH=""

CONTAINER_CPUS="$(apple_container_cpus)"

add_feature() {
  local feature="$1"
  local existing
  for existing in "${FEATURES[@]}"; do
    if [ "$existing" = "$feature" ]; then
      return
    fi
  done
  FEATURES+=("$feature")
}

usage() {
  cat <<'EOF'
Native Apple-container ARMv7 build:
  ./build-arm64-apple-container.sh              → release-device
  ./build-arm64-apple-container.sh --device     → release-device
  ./build-arm64-apple-container.sh --fast       → release (thin LTO, optimized daily deploy)
  ./build-arm64-apple-container.sh --all-scenes → compile bench scenes + experiments
  ./build-arm64-apple-container.sh --experiments → compile experimental effect scenes
  ./build-arm64-apple-container.sh --diagnostics → include diagnostics commands
  ./build-arm64-apple-container.sh --bench-tools → include device benchmark commands
  ./build-arm64-apple-container.sh --catalog-builder → build only the Slint-free catalog builder
  ./build-arm64-apple-container.sh --check       → check ARM UI without producing a binary
  ./build-arm64-apple-container.sh --check --lib-only → check the Slint-free ARM library
  ./build-arm64-apple-container.sh --ui-scope S → launcher | arcade | all
  ./build-arm64-apple-container.sh --clean      → clear the Apple-container target cache first
  ./build-arm64-apple-container.sh --rebuild-image → rebuild the cross image
  ./build-arm64-apple-container.sh --verbose    → show Cargo fingerprint detail

One-time host setup:
  rustup toolchain add stable-aarch64-unknown-linux-gnu --profile minimal --force-non-host
  rustup target add armv7-unknown-linux-gnueabihf --toolchain stable-aarch64-unknown-linux-gnu
  container system start
  container builder start --cpus "$(getconf _NPROCESSORS_ONLN)" --memory 8g

The Apple builder VM must be restarted with the full CPU/memory allocation before
a run can use it.
EOF
}

ARGS=("$@")
for ((i = 0; i < ${#ARGS[@]}; i++)); do
  arg="${ARGS[$i]}"
  case "$arg" in
    --device|--release-device) PROFILE=release-device ;;
    --fast) PROFILE=release ;;
    --check) COMMAND=check ;;
    --lib-only) LIB_ONLY=1 ;;
    --profile)
      PROFILE=release-device-profile
      add_feature profile
      ;;
    --diagnostics) add_feature diagnostics ;;
    --bench-tools) add_feature bench-tools ;;
    --catalog-builder)
      FEATURES=(builder)
      BIN_TARGET="mister-magik-catalog-builder"
      BIN_NAME="mister-magik-catalog-builder"
      MANIFEST_PATH="../../crates/catalog/Cargo.toml"
      UI_SCOPE=all
      UI_SCOPE_EXPLICIT=1
      ;;
    --all-scenes) UI_SCOPE=all; UI_SCOPE_EXPLICIT=1; add_feature experiments ;;
    --experiments) UI_SCOPE=all; UI_SCOPE_EXPLICIT=1; add_feature experiments ;;
    --ui-scope=*) UI_SCOPE="${arg#--ui-scope=}"; UI_SCOPE_EXPLICIT=1 ;;
    --ui-scope)
      i=$((i + 1))
      if [ "$i" -ge "${#ARGS[@]}" ]; then
        echo "ERROR: --ui-scope requires one of: launcher, arcade, all" >&2
        exit 2
      fi
      UI_SCOPE="${ARGS[$i]}"
      UI_SCOPE_EXPLICIT=1
      ;;
    --clean) CLEAN=1 ;;
    --rebuild-image) REBUILD_IMAGE=1 ;;
    --unlocked) LOCKED=0 ;;
    --verbose) VERBOSE=1 ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument: $arg" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [ "$LIB_ONLY" -eq 1 ]; then
  if [ "$COMMAND" != check ]; then
    echo "ERROR: --lib-only requires --check." >&2
    exit 2
  fi
  FEATURES=()
  BIN_NAME=""
fi

if [ "$PROFILE" = release ] && [ "$UI_SCOPE_EXPLICIT" -eq 0 ]; then
  UI_SCOPE=launcher
elif [ -z "$UI_SCOPE" ]; then
  UI_SCOPE=all
fi
case "$UI_SCOPE" in
  launcher|arcade|all) ;;
  *)
    echo "ERROR: invalid UI scope: $UI_SCOPE (expected launcher, arcade, or all)" >&2
    exit 2
    ;;
esac
export MISTER_UI_BUILD_SCOPE="$UI_SCOPE"
MISTER_MAGIK_BUILD_NUMBER="${MISTER_MAGIK_BUILD_NUMBER:-$(
  git -C "$PWD/.." rev-list --count HEAD 2>/dev/null || echo unknown
)}"
MISTER_MAGIK_VERSION="${MISTER_MAGIK_VERSION:-0.2.$MISTER_MAGIK_BUILD_NUMBER}"
if [ -z "${MISTER_MAGIK_BUILD_TIME:-}" ]; then
  MISTER_MAGIK_BUILD_TIME="$(
    git -C "$REPO_ROOT" show -s --format='%cd' --date='format:%-d.%-m.%Y %H:%M' HEAD 2>/dev/null || true
  )"
  MISTER_MAGIK_BUILD_TIME="${MISTER_MAGIK_BUILD_TIME:-unknown}"
fi

if [ "$(uname -m)" != arm64 ]; then
  echo "ERROR: Apple-container native path requires an arm64 macOS host; got $(uname -m)." >&2
  exit 1
fi
if ! command -v container >/dev/null 2>&1; then
  echo "ERROR: Apple container is not installed or not on PATH." >&2
  exit 1
fi
apple_container_warn_builder_resources "$(container builder status 2>/dev/null || true)" "$CONTAINER_CPUS"
if [ ! -x "$RUST_TOOLCHAIN/bin/cargo" ]; then
  echo "ERROR: missing linux/aarch64 Rust toolchain at $RUST_TOOLCHAIN" >&2
  echo "Install it with:" >&2
  echo "  rustup toolchain add stable-aarch64-unknown-linux-gnu --profile minimal --force-non-host" >&2
  echo "  rustup target add $TARGET --toolchain stable-aarch64-unknown-linux-gnu" >&2
  exit 1
fi
if [ ! -d "$RUST_TOOLCHAIN/lib/rustlib/$TARGET" ]; then
  echo "ERROR: missing $TARGET std for stable-aarch64-unknown-linux-gnu" >&2
  echo "Install it with:" >&2
  echo "  rustup target add $TARGET --toolchain stable-aarch64-unknown-linux-gnu" >&2
  exit 1
fi

if [ "$CLEAN" -eq 1 ]; then
  echo "==> clearing Apple-container target cache: $TARGET_DIR"
  rm -rf "$TARGET_DIR"
  rm -rf "$MIRROR_TARGET_DIR/$TARGET"
fi
mkdir -p "$TARGET_DIR" "$CARGO_CACHE"

dockerfile_hash() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$DOCKERFILE" | awk '{print $1}'
  else
    sha256sum "$DOCKERFILE" | awk '{print $1}'
  fi
}

image_stamp() {
  printf '%s  %s\n' "$IMAGE" "$(dockerfile_hash)"
}

image_exists() {
  echo 'WORKFLOW_COUNT kind=container phase=image-lookup'
  [ "$(container run --arch arm64 --rm "$IMAGE" uname -m 2>/dev/null || true)" = aarch64 ]
}

ensure_image() {
  local expected_stamp existing_stamp reason
  expected_stamp="$(image_stamp)"
  existing_stamp="$(cat "$IMAGE_STAMP" 2>/dev/null || true)"

  if [[ "$REBUILD_IMAGE" =~ ^(1|true|yes)$ ]]; then
    reason="requested"
  elif [ "$existing_stamp" != "$expected_stamp" ]; then
    reason="missing or stale Dockerfile stamp"
  elif ! image_exists; then
    reason="image not found"
  else
    echo "==> linux/arm64 cross image is current: $IMAGE"
    return
  fi

  echo "==> building linux/arm64 cross image: $IMAGE ($reason)"
  container build --arch arm64 --file "$DOCKERFILE" --tag "$IMAGE" .
  printf '%s\n' "$expected_stamp" >"$IMAGE_STAMP"
}

echo "==> host arch: $(uname -m)"
echo "==> container tool: $(container --version 2>&1 | head -n 1)"
echo "==> rust toolchain: $RUST_TOOLCHAIN"
echo "==> target triple: $TARGET"
echo "==> build backend: apple-container"
echo "==> build CPUs: $CONTAINER_CPUS"
echo "==> build memory: $CONTAINER_MEMORY"
phase_start image-lookup
ensure_image
phase_end image-lookup

FEATURE_LIST="$(IFS=,; echo "${FEATURES[*]}")"
BUILD_ARGS=("$COMMAND" --target "$TARGET")
if [ "$COMMAND" = build ]; then
  BUILD_ARGS+=(--profile "$PROFILE")
fi
if [ -n "$FEATURE_LIST" ]; then
  BUILD_ARGS+=(--features "$FEATURE_LIST")
fi
if [ "$LIB_ONLY" -eq 1 ]; then
  BUILD_ARGS+=(--lib --no-default-features)
fi
if [ -n "$MANIFEST_PATH" ]; then
  BUILD_ARGS+=(--manifest-path "$MANIFEST_PATH")
fi
if [ "$LOCKED" -eq 1 ]; then
  BUILD_ARGS+=(--locked)
fi
if [ "$VERBOSE" -eq 1 ]; then
  BUILD_ARGS+=(-vv)
fi
if [ -n "$BIN_TARGET" ]; then
  BUILD_ARGS+=(--bin "$BIN_TARGET")
fi

EXTRA_ENVS=()
if [ "$LIB_ONLY" -eq 0 ] && [ "$BIN_NAME" = "mister-magik-fb" ]; then
  phase_start ffmpeg-cache-check
  echo 'WORKFLOW_COUNT kind=ffmpeg-cache-check phase=ffmpeg-cache-check'
  bash "$PWD/scripts/build-minimal-ffmpeg.sh"
  phase_end ffmpeg-cache-check
  EXTRA_ENVS+=(
    --env FFMPEG_DIR=/project/apps/mister/target/ffmpeg-minimal/armv7/dist
    --env PKG_CONFIG_PATH=/project/apps/mister/target/ffmpeg-minimal/armv7/dist/lib/pkgconfig
    --env PKG_CONFIG_ALLOW_CROSS=1
    --env CFLAGS=-I/project/apps/mister/target/ffmpeg-minimal/armv7/dist/include
    --env HOST_CFLAGS=-I/project/apps/mister/target/ffmpeg-minimal/armv7/dist/include
    --env CFLAGS_aarch64_unknown_linux_gnu=-I/project/apps/mister/target/ffmpeg-minimal/armv7/dist/include
  )
  echo "==> using minimal FFmpeg: /project/apps/mister/target/ffmpeg-minimal/armv7/dist"
fi

HOST_RUSTFLAGS="${RUSTFLAGS:-}"
CONTAINER_RUSTFLAGS="${HOST_RUSTFLAGS:+$HOST_RUSTFLAGS }-D warnings -C target-cpu=cortex-a9"
if [ "$PROFILE" = release-device-profile ]; then
  CONTAINER_RUSTFLAGS="$CONTAINER_RUSTFLAGS -C force-frame-pointers=yes"
fi

echo "==> container build profile=$PROFILE ui_scope=$UI_SCOPE features=$FEATURE_LIST"
echo "==> target dir: $TARGET_DIR"
BUILD_METADATA_ENVS=(
  --env MISTER_MAGIK_BUILD_NUMBER="$MISTER_MAGIK_BUILD_NUMBER"
  --env MISTER_MAGIK_VERSION="$MISTER_MAGIK_VERSION"
  --env MISTER_MAGIK_BUILD_TIME="$MISTER_MAGIK_BUILD_TIME"
)
phase_start cargo-container
echo 'WORKFLOW_COUNT kind=container phase=cargo-container'
container run --arch arm64 --rm \
  --cpus "$CONTAINER_CPUS" \
  --memory "$CONTAINER_MEMORY" \
  --env CARGO_HOME=/cargo \
  --env CARGO_TARGET_DIR=/target \
  --env CARGO_BUILD_JOBS="$CONTAINER_CPUS" \
  --env CMAKE_BUILD_PARALLEL_LEVEL="$CONTAINER_CPUS" \
  --env MAKEFLAGS="-j$CONTAINER_CPUS" \
  --env MISTER_UI_BUILD_SCOPE="$UI_SCOPE" \
  "${BUILD_METADATA_ENVS[@]}" \
  --env RUSTC_WRAPPER= \
  --env RUSTFLAGS="$CONTAINER_RUSTFLAGS" \
  --env SLINT_FONT_SIZES="${SLINT_FONT_SIZES:-8,16,24,32}" \
  "${EXTRA_ENVS[@]}" \
  --volume "$CARGO_CACHE:/cargo" \
  --volume "$RUST_TOOLCHAIN:/rust:ro" \
  --volume "$REPO_ROOT:/project" \
  --volume "$TARGET_DIR:/target" \
  --workdir /project/apps/mister \
  "$IMAGE" \
  sh -lc 'PATH=/rust/bin:$PATH cargo "$@"' sh "${BUILD_ARGS[@]}"
phase_end cargo-container

if [ "$COMMAND" = check ]; then
  echo "==> check OK (no binary produced)"
  exit 0
fi

BIN="$TARGET_DIR/$TARGET/$PROFILE/$BIN_NAME"
if [ ! -f "$BIN" ]; then
  echo "ERROR: expected binary not found: $BIN" >&2
  exit 1
fi

MIRROR_BIN="$MIRROR_TARGET_DIR/$TARGET/$PROFILE/$BIN_NAME"
phase_start artifact-mirror
mkdir -p "$(dirname "$MIRROR_BIN")"
cp "$BIN" "$MIRROR_BIN"
printf '%s\n' "$FEATURE_LIST" >"$MIRROR_BIN.features"
phase_end artifact-mirror
source "$PWD/../../scripts/lib/bench-context-lib.sh"
phase_start build-receipt
bench_context_write_build_receipt "$MIRROR_BIN" "$REPO_ROOT" "$PROFILE" "$FEATURE_LIST" "$UI_SCOPE"
phase_end build-receipt

BYTES="$(stat -f%z "$BIN" 2>/dev/null || stat -c%s "$BIN")"
echo "==> build OK: $BIN"
echo "==> mirrored binary: $MIRROR_BIN"
echo "==> binary size: $BYTES bytes"
phase_start size-record
"$PWD/scripts/record-binary-size.sh" "$PROFILE" "${FEATURE_LIST:-none}" "$MIRROR_BIN"
phase_end size-record
