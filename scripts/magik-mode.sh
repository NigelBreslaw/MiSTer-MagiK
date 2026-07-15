#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Select stock, public MagiK, or development MagiK without mixing their files.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="${MISTER:-$ROOT/scripts/mister}"
source "$ROOT/scripts/reboot-wait-lib.sh"

usage() {
  echo "usage: scripts/magik-mode.sh <status|dev|public|stock>"
}

verify_layout() {
  local layout="$1" app main
  case "$layout" in
    dev) app=/media/fat/mister-magik-dev; main=/media/fat/MiSTer_MagiKDev ;;
    public) app=/media/fat/mister-magik; main=/media/fat/MiSTer_MagiK ;;
    *) return 2 ;;
  esac
  "$MISTER" run "
set -e
manifest='$app/platform-v2.manifest'
get() { value=\$(sed -n \"s/^\$1=//p\" \"\$manifest\"); test -n \"\$value\"; test \"\$(grep -c \"^\$1=\" \"\$manifest\")\" -eq 1; printf '%s' \"\$value\"; }
test \"\$(get format)\" = mister-magik-platform-v2
test \"\$(get main_path)\" = '$main'
test \"\$(get gui_path)\" = '$app/mister-magik-fb'
test \"\$(get scanout_module_path)\" = '$app/mister_magik_scanout_slots.ko'
test \"\$(get scanout_metadata_path)\" = '$app/mister_magik_scanout_slots.metadata.txt'
test \"\$(get latch_rbf_path)\" = '$app/fpga/menu-magik-vblank-latch.rbf'
test \"\$(get latch_metadata_path)\" = '$app/fpga/menu-magik-vblank-latch.metadata.txt'
check() { path=\$1; key=\$2; test -r \"\$path\"; test \"\$(sha256sum \"\$path\" | awk '{print \$1}')\" = \"\$(get \"\$key\")\"; }
check '$main' main_sha256
check '$app/mister-magik-fb' gui_sha256
check '$app/mister_magik_scanout_slots.ko' scanout_module_sha256
check '$app/mister_magik_scanout_slots.metadata.txt' scanout_metadata_sha256
check '$app/fpga/menu-magik-vblank-latch.rbf' latch_rbf_sha256
check '$app/fpga/menu-magik-vblank-latch.metadata.txt' latch_metadata_sha256
contract=\$(get platform_contract_sha256)
module_hash=\$(get scanout_module_sha256)
rbf_hash=\$(get latch_rbf_sha256)
grep -qx "platform_contract_sha256=\$contract" '$app/mister_magik_scanout_slots.metadata.txt'
grep -qx "platform_contract_sha256=\$contract" '$app/fpga/menu-magik-vblank-latch.metadata.txt'
grep -qx "module_sha256=\$module_hash" '$app/mister_magik_scanout_slots.metadata.txt'
grep -qx "rbf_sha256=\$rbf_hash" '$app/fpga/menu-magik-vblank-latch.metadata.txt'
echo '$layout platform valid'
"
}

clear_arming_state() {
  "$MISTER" run "rm -f \
    /media/fat/mister-magik/launcher.env \
    /media/fat/mister-magik-dev/launcher.env \
    /media/fat/mister-magik/rebuild-on-next-boot \
    /media/fat/mister-magik-dev/rebuild-on-next-boot \
    /tmp/mister-magik/fs-fault-launcher.env \
    /tmp/mister-magik/fs-fault-session \
    /tmp/mister-magik/fs-fault.json; sync"
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
