#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Prove two unchanged production builds remain no-ops after one warm-up build.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SKIP_WARMUP=0
VERIFY_METADATA=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --skip-warmup) SKIP_WARMUP=1 ;;
    --verify-metadata) VERIFY_METADATA=1 ;;
    *) echo "usage: scripts/regression-arm-noop.sh [--skip-warmup] [--verify-metadata]" >&2; exit 2 ;;
  esac
  shift
done

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

run_build() {
  local label="$1"
  "$ROOT/apps/mister/build-arm.sh" --device 2>&1 | tee "$TMP/$label.log"
}

if [ "$SKIP_WARMUP" -eq 0 ]; then
  run_build warmup
fi
run_build measured-1
run_build measured-2

for log in "$TMP/measured-1.log" "$TMP/measured-2.log"; do
  if grep -Eq 'Compiling mister-magik-fb|Checking mister-magik-fb' "$log"; then
    echo "ERROR: unchanged ARM build was not a no-op: $log" >&2
    exit 1
  fi
done
echo "ARM production no-op regression check ok"

if [ "$VERIFY_METADATA" -eq 1 ]; then
  expected="$(git -C "$ROOT" show -s --format='%cd' --date='format:%-d.%-m.%Y %H:%M' HEAD)"
  binary="$ROOT/apps/mister/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
  strings "$binary" >"$TMP/binary.strings"
  if ! grep -Fq "$expected" "$TMP/binary.strings"; then
    echo "ERROR: production binary does not embed the checked-out commit timestamp" >&2
    exit 1
  fi

  "$ROOT/apps/mister/build-arm.sh" --check --ui-scope launcher >"$TMP/metadata-warm.log" 2>&1
  override="metadata-override-regression"
  MISTER_MAGIK_BUILD_TIME="$override" \
    "$ROOT/apps/mister/build-arm.sh" --check --ui-scope launcher >"$TMP/metadata-override-1.log" 2>&1
  MISTER_MAGIK_BUILD_TIME="$override" \
    "$ROOT/apps/mister/build-arm.sh" --check --ui-scope launcher >"$TMP/metadata-override-2.log" 2>&1
  if ! grep -Eq 'Compiling mister-magik-fb|Checking mister-magik-fb' "$TMP/metadata-override-1.log"; then
    echo "ERROR: explicit build-time override did not invalidate Cargo" >&2
    exit 1
  fi
  if grep -Eq 'Compiling mister-magik-fb|Checking mister-magik-fb' "$TMP/metadata-override-2.log"; then
    echo "ERROR: unchanged explicit build-time override invalidated Cargo twice" >&2
    exit 1
  fi
  "$ROOT/apps/mister/build-arm.sh" --check --ui-scope launcher >"$TMP/metadata-restored.log" 2>&1
  echo "ARM build metadata regression check ok"
fi
