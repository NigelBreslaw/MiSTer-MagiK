#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Select stock, public MagiK, or development MagiK without mixing their files.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="${MISTER:-$ROOT/scripts/mister}"
source "$ROOT/scripts/lib/reboot-wait-lib.sh"
source "$ROOT/scripts/lib/arming-state-lib.sh"
source "$ROOT/scripts/lib/platform-manifest-lib.sh"

usage() {
  echo "usage: scripts/magik-mode.sh <status|dev|public|stock>"
}

verify_layout() {
  local layout="$1" app
  case "$layout" in
    dev) app=/media/fat/mister-magik-dev ;;
    public) app=/media/fat/mister-magik ;;
    *) return 2 ;;
  esac
  platform_manifest_verify "$MISTER" "$layout" "$app/platform-v2.manifest"
  echo "$layout platform valid"
}

clear_arming_state() {
  arming_state_clear "$MISTER"
  arming_state_assert_clean "$MISTER"
}

status() {
  "$MISTER" run "
echo -n 'selected_main='
awk 'BEGIN{s=0} /^\\[MiSTer\\]/{s=1;next} /^\\[/{s=0} s && /^[[:space:]]*main[[:space:]]*=/{v=\$0;sub(/^[^=]*=[[:space:]]*/,\"\",v);sub(/[[:space:]]*[;#].*$/,\"\",v);print v;found=1} END{if(!found)print \"MiSTer\"}' /media/fat/MiSTer.ini
echo -n 'running_main='
running=none
for name in MiSTer_MagiKDev MiSTer_MagiK MiSTer; do
  if pidof "\$name" >/dev/null 2>&1; then
    pids=\$(pidof "\$name" 2>/dev/null || true)
    running="\$name:\$pids"
    break
  fi
done
echo "\$running"
"
  for layout in public dev; do
    if verify_layout "$layout" >/dev/null 2>&1; then
      echo "$layout=valid"
    else
      echo "$layout=missing-or-invalid"
    fi
  done
}

switch_mode() {
  local mode="$1"
  case "$mode" in
    dev)
      verify_layout dev
      "$MISTER" inittab-ensure-stock
      "$MISTER" ini-repair-boot
      "$MISTER" ini-select-main MiSTer_MagiKDev
      ;;
    public)
      verify_layout public
      "$MISTER" run "test -s /media/fat/MiSTer.ini.bak.before-magik || { echo 'Public files are downloaded but not activated; run Scripts -> mister-magik.' >&2; exit 14; }"
      "$MISTER" inittab-ensure-stock
      "$MISTER" ini-repair-boot
      ;;
    stock)
      "$MISTER" inittab-ensure-stock
      "$MISTER" ini-select-main MiSTer
      ;;
  esac
  clear_arming_state
  MISTER="$MISTER" mister_reboot_wait_with_raw_fallback
}

case "${1:-}" in
  status) status ;;
  dev|public|stock) switch_mode "$1" ;;
  -h|--help|"") usage ;;
  *) usage >&2; exit 2 ;;
esac
