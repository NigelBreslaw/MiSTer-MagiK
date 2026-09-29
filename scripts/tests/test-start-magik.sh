#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

source "$ROOT/scripts/lib/shared-worktree-cache.sh"
MANAGER_TARGET="${CARGO_TARGET_DIR:-$(shared_primary_checkout "$ROOT")/mister/tools/manager/target}"
MANAGER_BINARY="$MANAGER_TARGET/debug/mister-magik-manager"
[ -x "$MANAGER_BINARY" ] || {
  echo "missing manager binary: $MANAGER_BINARY (run cargo build first)" >&2
  exit 1
}

FAT="$TMP/fat"
APP="$FAT/mister-magik"
mkdir -p "$APP/fpga" "$FAT/Scripts"
printf '[MiSTer]\r\nmain=MiSTer\r\n[Menu]\r\nvideo_mode=8\r\n' >"$FAT/MiSTer.ini"
SHA256SUM="$(command -v sha256sum)"
BEFORE_INI="$("$SHA256SUM" "$FAT/MiSTer.ini")"

# The manager requires a Main carrying the session guard.
printf '#!/bin/sh\n# MISTER_MAGIK_SESSION_MAIN\n' >"$FAT/MiSTer_MagiK"
printf '#!/bin/sh\n' >"$APP/mister-magik-fb"
cp "$MANAGER_BINARY" "$APP/mister-magik-manager"
chmod 755 "$APP/mister-magik-manager"
printf 'module\n' >"$APP/mister_magik_scanout_slots.ko"
printf 'rbf\n' >"$APP/fpga/menu-magik-vblank-latch.rbf"
contract="$(printf contract | sha256sum | awk '{print $1}')"
module_hash="$(sha256sum "$APP/mister_magik_scanout_slots.ko" | awk '{print $1}')"
rbf_hash="$(sha256sum "$APP/fpga/menu-magik-vblank-latch.rbf" | awk '{print $1}')"
# A public-layout platform is a legacy 5.15 module; test mode supplies the kernel.
printf 'platform_contract_sha256=%s\nmodule_sha256=%s\nvermagic=5.15.1-MiSTer SMP mod_unload ARMv7 p2v8 \n' \
  "$contract" "$module_hash" >"$APP/mister_magik_scanout_slots.metadata.txt"
printf 'platform_contract_sha256=%s\nsource_commit=%040d\nlatch_protocol_version=5\nlatch_capability_mask=0x03ff\nrbf_sha256=%s\n' \
  "$contract" 3 "$rbf_hash" >"$APP/fpga/menu-magik-vblank-latch.metadata.txt"
printf '{"format":"mister-magik-platform-bundle-v0.2","release_version":16,"bundle_id":"%064d"}\n' 0 \
  >"$APP/platform-bundle-v0.2.json"
"$ROOT/scripts/magik-ci" ci platform-manifest generate \
  --output "$APP/platform-v3.manifest" --layout public \
  --main "$FAT/MiSTer_MagiK" --gui "$APP/mister-magik-fb" \
  --manager "$APP/mister-magik-manager" \
  --scanout-module "$APP/mister_magik_scanout_slots.ko" \
  --scanout-metadata "$APP/mister_magik_scanout_slots.metadata.txt" \
  --latch-rbf "$APP/fpga/menu-magik-vblank-latch.rbf" \
  --latch-metadata "$APP/fpga/menu-magik-vblank-latch.metadata.txt" \
  --platform-bundle-manifest "$APP/platform-bundle-v0.2.json" \
  --main-revision "$(printf %040d 2)" --magik-revision "$(printf %040d 1)" >/dev/null
cp "$ROOT/scripts/Start_MagiK.sh" "$FAT/Scripts/Start_MagiK.sh"
chmod 755 "$FAT/Scripts/Start_MagiK.sh"

run_start() {
  MISTER_MAGIK_FAT="$FAT" \
    MISTER_MAGIK_TEST_MODE=1 \
    MISTER_MAGIK_TEST_KERNEL_RELEASE=5.15.1-MiSTer \
    MISTER_MAGIK_TEST_KEYS="${MISTER_MAGIK_TEST_KEYS:-}" \
    "$FAT/Scripts/Start_MagiK.sh" </dev/null
}

expect_refused() {
  local name="$1" pattern="$2"
  if run_start >"$TMP/$name.log" 2>&1; then
    echo "$name unexpectedly started" >&2
    exit 1
  fi
  grep -q "$pattern" "$TMP/$name.log"
  test "$("$SHA256SUM" "$FAT/MiSTer.ini")" = "$BEFORE_INI"
}

# Only Down confirms; anything else cancels without a handoff.
MISTER_MAGIK_TEST_KEYS=enter expect_refused cancel 'start cancelled'
! grep -q 'live handoff requested' "$TMP/cancel.log"

# A missing hashing tool, manager, or corrupt manager never reaches the manager.
mkdir "$TMP/bin"
for tool in grep sed awk chmod env; do
  ln -s "$(command -v "$tool")" "$TMP/bin/$tool"
done
PATH="$TMP/bin" MISTER_MAGIK_TEST_KEYS=down expect_refused missing-tool \
  'required tool is unavailable: sha256sum'

mv "$APP/mister-magik-manager" "$TMP/manager.good"
MISTER_MAGIK_TEST_KEYS=down expect_refused missing-manager 'missing .*mister-magik-manager'
cp "$TMP/manager.good" "$APP/mister-magik-manager"
printf 'corrupt\n' >>"$APP/mister-magik-manager"
MISTER_MAGIK_TEST_KEYS=down expect_refused corrupt-manager 'manager hash mismatch'
cp "$TMP/manager.good" "$APP/mister-magik-manager"

# Confirmation verifies the public platform and requests the live handoff,
# leaving MiSTer.ini and the next boot unchanged.
MISTER_MAGIK_TEST_KEYS=down run_start >"$TMP/start.log"
grep -q 'verified platform' "$TMP/start.log"
grep -q 'TEST: live handoff requested' "$TMP/start.log"
grep -q 'starting MiSTer_MagiK in' "$TMP/start.log"
test "$("$SHA256SUM" "$FAT/MiSTer.ini")" = "$BEFORE_INI"

echo "Start_MagiK tests passed"
