#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
REMOTE_DIR="/tmp/mister-magik-scanout-slots"
REMOTE_KO="$REMOTE_DIR/mister_magik_scanout_slots.ko"
REMOTE_BIN="/media/fat/mister-magik-dev/mister-magik-fb"
LOCAL_KO="$ROOT/build/scanout-slots/mister_magik_scanout_slots.ko"
LOCAL_REPORT="$ROOT/build/scanout-slots/scanout-slots-map-report.log"

cleanup() {
  "$MISTER" run "rmmod mister_magik_scanout_slots 2>/dev/null || true; rm -rf '$REMOTE_DIR'" >/dev/null 2>&1 || true
}
trap cleanup EXIT

mkdir -p "$ROOT/build/scanout-slots"

"$ROOT/scripts/build-scanout-slots-module.sh"
"$ROOT/scripts/agent" deploy-recipe all-diagnostics-device

test -f "$LOCAL_KO"
if ! grep -q 'vermagic:.*5\.15\.1-MiSTer' "$ROOT/build/scanout-slots/modinfo.txt"; then
  echo "module vermagic does not target 5.15.1-MiSTer:" >&2
  cat "$ROOT/build/scanout-slots/modinfo.txt" >&2
  exit 1
fi

echo "==> Verifying stock kernel and module tools"
"$MISTER" run "set -e; uname -r; test \"\$(uname -r)\" = '5.15.1-MiSTer'; command -v insmod; command -v rmmod; command -v modprobe >/dev/null"

echo "==> Clearing any old scanout-slots module"
cleanup

echo "==> Uploading diagnostics binary and scanout-slots module"
"$MISTER" run "mkdir -p '$REMOTE_DIR'"
"$MISTER" put "$LOCAL_KO" "$REMOTE_KO"
"$MISTER" run "chmod 600 '$REMOTE_KO'; sync"

echo "==> Loading module"
"$MISTER" run "insmod '$REMOTE_KO'; test -e /dev/mister-magik-scanout-slots; grep '^mister_magik_scanout_slots ' /proc/modules"

echo "==> Running scanout-slots-map-report"
"$MISTER" run "'$REMOTE_BIN' scanout-slots-map-report" | tee "$LOCAL_REPORT"

echo "==> Unloading module and restoring normal MagiK"
cleanup
"$ROOT/scripts/agent" deploy
"$ROOT/scripts/run-rust.sh" launcher 0
"$MISTER" run "set -e; for i in \$(seq 1 20); do pidof mister-magik-fb >/dev/null 2>&1 && break; sleep 0.5; done; ! test -e /dev/mister-magik-scanout-slots; ! grep -q '^mister_magik_scanout_slots ' /proc/modules; pidof MiSTer_MagiKDev; pidof mister-magik-fb; ls -l /media/fat/mister-magik-dev/launcher.env /tmp/mister-magik/fs-fault* /media/fat/mister-magik-dev/rebuild-on-next-boot 2>/dev/null || true"

echo "==> Wrote:"
echo "    $LOCAL_REPORT"
