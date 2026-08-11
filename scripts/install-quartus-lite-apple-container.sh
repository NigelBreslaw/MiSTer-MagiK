#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LOCAL_ROOT="${MISTER_FPGA_LOCAL_ROOT:-$ROOT/build/fpga-local-apple}"
CACHE_DIR="$LOCAL_ROOT/quartus-lite-17.0"
INSTALL_ROOT="$CACHE_DIR/apple-intelFPGA_lite"
RUNTIME_IMAGE="${QUARTUS_APPLE_IMAGE:-mister-magik-quartus17-apple:ubuntu18-amd64}"
INSTALLER_IMAGE="${QUARTUS_APPLE_INSTALLER_IMAGE:-mister-magik-quartus17-installer:ubuntu20-arm64}"
INSTALLER_VOLUME="${QUARTUS_APPLE_INSTALLER_VOLUME:-mister-magik-quartus17-installer-root-v1}"
QUARTUS_RUN="QuartusLiteSetup-17.0.0.595-linux.run"
CYCLONEV_QDZ="cyclonev-17.0.0.595.qdz"
QUARTUS_URL="${QUARTUS_17_0_RUN_URL:-https://downloads.intel.com/akdlm/software/acdsinst/17.0std/595/ib_installers/$QUARTUS_RUN}"
CYCLONEV_URL="${QUARTUS_17_0_CYCLONEV_QDZ_URL:-https://downloads.intel.com/akdlm/software/acdsinst/17.0std/595/ib_installers/$CYCLONEV_QDZ}"

usage() {
  cat <<'EOF'
Usage:
  QUARTUS_ACCEPT_EULA=1 scripts/agent fpga setup

Installs the pinned Quartus Prime Lite 17.0 Build 595 runtime for the local
Apple Silicon FPGA signoff workflow. Apple Container Rosetta runs Quartus;
an amd64 QEMU chroot is used only for the official installer, which cannot
complete under Rosetta.

Set MISTER_FPGA_LOCAL_ROOT to place the reusable install and signoff cache in a
stable directory shared by multiple Git worktrees.
EOF
}

case "${1:-}" in
  -h|--help)
    usage
    exit 0
    ;;
  "") ;;
  *)
    echo "unknown argument: $1" >&2
    usage >&2
    exit 2
    ;;
esac

command -v container >/dev/null 2>&1 || {
  echo "Apple container is required" >&2
  exit 1
}

runtime_ready=0
if [[ -x "$INSTALL_ROOT/17.0/quartus/bin/quartus_sh" ]] &&
  container image inspect "$RUNTIME_IMAGE" >/dev/null 2>&1; then
  runtime_ready=1
fi

if [[ "$runtime_ready" = 1 ]]; then
  container run --arch amd64 --rm \
    --mount "type=bind,source=$INSTALL_ROOT,target=/opt/intelFPGA_lite,readonly" \
    "$RUNTIME_IMAGE" quartus_sh --version
  exit 0
fi

if [[ "${QUARTUS_ACCEPT_EULA:-}" != 1 ]]; then
  echo "set QUARTUS_ACCEPT_EULA=1 after accepting the Quartus installer terms" >&2
  exit 1
fi

mkdir -p "$CACHE_DIR" "$INSTALL_ROOT"

download() {
  local destination="$1"
  local url="$2"
  [[ -f "$destination" ]] || curl --fail --location --retry 5 --output "$destination" "$url"
}

verify_sha1() {
  local path="$1"
  local expected="$2"
  local actual
  actual="$(shasum -a 1 "$path" | awk '{print $1}')"
  if [[ "$actual" != "$expected" ]]; then
    echo "sha1 mismatch for $path: expected $expected, got $actual" >&2
    exit 1
  fi
}

download "$CACHE_DIR/$QUARTUS_RUN" "$QUARTUS_URL"
download "$CACHE_DIR/$CYCLONEV_QDZ" "$CYCLONEV_URL"
verify_sha1 "$CACHE_DIR/$QUARTUS_RUN" 99ccfb15962febceba64de2dc9b28c47e5a3b8df
verify_sha1 "$CACHE_DIR/$CYCLONEV_QDZ" 2198dedb99866f38d43ff6c029d4bd668e2bbb59

if ! container image inspect "$RUNTIME_IMAGE" >/dev/null 2>&1; then
  container build --arch amd64 \
    --file "$ROOT/scripts/quartus/apple-runtime.Containerfile" \
    --tag "$RUNTIME_IMAGE" "$ROOT/scripts/quartus"
fi
if ! container image inspect "$INSTALLER_IMAGE" >/dev/null 2>&1; then
  container build --arch arm64 \
    --file "$ROOT/scripts/quartus/apple-installer.Containerfile" \
    --tag "$INSTALLER_IMAGE" "$ROOT/scripts/quartus"
fi
if ! container volume inspect "$INSTALLER_VOLUME" >/dev/null 2>&1; then
  container volume create "$INSTALLER_VOLUME"
fi

container run --rm \
  --mount "type=volume,source=$INSTALLER_VOLUME,target=/qemu-root" \
  "$INSTALLER_IMAGE" sh -lc '
    set -eu
    if [ ! -f /qemu-root/etc/os-release ]; then
      debootstrap --arch=amd64 --foreign bionic /qemu-root http://archive.ubuntu.com/ubuntu
      cp /usr/bin/qemu-x86_64-static /qemu-root/usr/bin/qemu-x86_64-static
      chroot /qemu-root /usr/bin/qemu-x86_64-static \
        /bin/sh /debootstrap/debootstrap --second-stage
    fi
    test "$(sed -n "s/^VERSION_ID=//p" /qemu-root/etc/os-release | tr -d "\"")" = 18.04
    test -x /qemu-root/usr/bin/qemu-x86_64-static
  '

if [[ ! -x "$INSTALL_ROOT/17.0/quartus/bin/quartus_sh" ]]; then
  container run --rm \
    --mount "type=volume,source=$INSTALLER_VOLUME,target=/qemu-root" \
    --mount "type=bind,source=$CACHE_DIR,target=/qemu-root/quartus-cache" \
    --mount "type=bind,source=$INSTALL_ROOT,target=/qemu-root/opt/intelFPGA_lite" \
    "$INSTALLER_IMAGE" sh -lc '
      set -eu
      chmod +x /qemu-root/quartus-cache/QuartusLiteSetup-17.0.0.595-linux.run
      chroot /qemu-root /usr/bin/qemu-x86_64-static /bin/bash -lc \
        "/quartus-cache/QuartusLiteSetup-17.0.0.595-linux.run --mode unattended --unattendedmodeui minimal --installdir /opt/intelFPGA_lite/17.0"
    '
fi

test -x "$INSTALL_ROOT/17.0/quartus/bin/quartus_sh"
container run --arch amd64 --rm \
  --mount "type=bind,source=$INSTALL_ROOT,target=/opt/intelFPGA_lite,readonly" \
  "$RUNTIME_IMAGE" quartus_sh --version
