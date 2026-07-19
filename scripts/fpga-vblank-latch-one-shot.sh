#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
REMOTE_BIN="/media/fat/mister-magik-dev/mister-magik-fb"
LOCAL_REPORT="$ROOT/build/fpga-vblank-latch/fpga-latch-report.log"

mkdir -p "$ROOT/build/fpga-vblank-latch"

"$ROOT/apps/mister/build-arm.sh" --diagnostics

echo "==> Verifying MiSTer is reachable and launcher boot is supervised"
"$MISTER" status

echo "==> Uploading diagnostics binary"
"$MISTER" agent deploy-magik-bin \
  "$ROOT/apps/mister/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb" \
  "$REMOTE_BIN"

echo "==> Running FPGA vblank latch capability report"
"$MISTER" run "'$REMOTE_BIN' fpga-latch-report" | tee "$LOCAL_REPORT"

echo "==> Restoring normal MagiK binary and launcher"
"$ROOT/scripts/deploy-rust.sh"
"$ROOT/scripts/run-rust.sh" launcher 0
"$MISTER" run "set -e; for i in \$(seq 1 20); do pidof mister-magik-fb >/dev/null 2>&1 && break; sleep 0.5; done; pidof MiSTer_MagiKDev; pidof mister-magik-fb; ls -l /media/fat/mister-magik-dev/launcher.env /tmp/mister-magik/fs-fault* /media/fat/mister-magik-dev/rebuild-on-next-boot 2>/dev/null || true"

echo "==> Wrote:"
echo "    $LOCAL_REPORT"
