#!/bin/sh
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# MiSTer MagiK installer for MiSTer Scripts menu / Downloader / update_all.

set -eu

FAT="${MISTER_MAGIK_FAT:-/media/fat}"
INITTAB="${MISTER_MAGIK_INITTAB:-/etc/inittab}"
INI="$FAT/MiSTer.ini"
INI_BACKUP="$FAT/MiSTer.ini.bak.before-magik"
INI_BACKUP_PENDING="$FAT/.MiSTer.ini.bak.before-magik.new.$$"
APP_DIR="$FAT/mister-magik"
MAIN_BIN="$FAT/MiSTer_MagiK"
GUI_BIN="$APP_DIR/mister-magik-fb"
CATALOG_BUILDER="$APP_DIR/mister-magik-catalog-builder"
SNAP_DIR="$APP_DIR/snapshots"
PENDING="$FAT/.MiSTer.ini.magik.new"
MANIFEST="$APP_DIR/platform-v1.manifest"
INSTALLED_SCRIPT="$FAT/Scripts/mister-magik.sh"
CHANNEL_SCRIPT="$FAT/Scripts/mister-magik-channel.sh"
DOWNLOADER_DROP_IN="$FAT/downloader_mister_magik.ini"

say() {
  echo "MiSTer MagiK: $*"
}

pause_exit() {
  [ "${MISTER_MAGIK_NO_PAUSE:-0}" = 1 ] && return 0
  echo
  echo "Press Enter to exit."
  read _ || true
}

ini_selects_magik() {
  [ -f "$INI" ] || return 1
  awk '
BEGIN { in_mister = 0; selected = "" }
function is_main_assignment(line, key) {
  if (line !~ /=/) return 0
  key = line
  sub(/=.*/, "", key)
  gsub(/[[:space:]]/, "", key)
  return tolower(key) == "main"
}
/^[[:space:]]*\[[^]]+\]/ {
  section = $0
  sub(/^[[:space:]]*\[/, "", section)
  sub(/\].*$/, "", section)
  gsub(/[[:space:]]/, "", section)
  in_mister = (tolower(section) == "mister")
  next
}
in_mister && is_main_assignment($0) {
  value = $0
  sub(/^[^=]*=[[:space:]]*/, "", value)
  sub(/[[:space:]]*[;#].*$/, "", value)
  gsub(/[[:space:]]/, "", value)
  selected = value
}
END { exit(selected == "MiSTer_MagiK" ? 0 : 1) }
' "$INI"
}

ini_has_one_main() {
  expected="$1"
  [ -f "$INI" ] || return 1
  awk -v expected="$expected" '
BEGIN { in_mister = 0; count = 0; selected = "" }
function is_main_assignment(line, key) {
  if (line !~ /=/) return 0
  key = line
  sub(/=.*/, "", key)
  gsub(/[[:space:]]/, "", key)
  return tolower(key) == "main"
}
/^[[:space:]]*\[[^]]+\]/ {
  section = $0
  sub(/^[[:space:]]*\[/, "", section)
  sub(/\].*$/, "", section)
  gsub(/[[:space:]]/, "", section)
  in_mister = (tolower(section) == "mister")
  next
}
in_mister && is_main_assignment($0) {
  value = $0
  sub(/^[^=]*=[[:space:]]*/, "", value)
  sub(/[[:space:]]*[;#].*$/, "", value)
  gsub(/[[:space:]]/, "", value)
  selected = value
  count++
}
END { exit(count == 1 && selected == expected ? 0 : 1) }
' "$INI"
}

backup_ini_before_magik() {
  [ -f "$INI" ] || return 0
  [ -e "$INI_BACKUP" ] && return 0
  if ini_selects_magik; then
    say "WARNING: $INI_BACKUP is missing; not creating it from a MagiK-active MiSTer.ini."
    return 0
  fi

  cp "$INI" "$INI_BACKUP_PENDING"
  sync "$INI_BACKUP_PENDING" 2>/dev/null || sync
  if [ -e "$INI_BACKUP" ]; then
    rm -f "$INI_BACKUP_PENDING"
  else
    mv "$INI_BACKUP_PENDING" "$INI_BACKUP"
    sync "$INI_BACKUP" 2>/dev/null || sync
    say "backup: $INI_BACKUP"
  fi
}

restore_menu_terminal() {
  if [ -n "${MENU_SAVED_STTY:-}" ]; then
    stty "$MENU_SAVED_STTY" 2>/dev/null || true
    MENU_SAVED_STTY=""
  fi
}

read_menu_key() {
  if [ "${MISTER_MAGIK_TEST_MODE:-0}" = 1 ] && [ -n "${MISTER_MAGIK_TEST_KEYS:-}" ]; then
    case "$MISTER_MAGIK_TEST_KEYS" in
      *,*) key="${MISTER_MAGIK_TEST_KEYS%%,*}"; MISTER_MAGIK_TEST_KEYS="${MISTER_MAGIK_TEST_KEYS#*,}" ;;
      *) key="$MISTER_MAGIK_TEST_KEYS"; MISTER_MAGIK_TEST_KEYS="" ;;
    esac
    case "$key" in
      up|down|enter|cancel|other) MENU_KEY="$key" ;;
      *) MENU_KEY=other ;;
    esac
    return 0
  fi
  [ -t 0 ] || return 1
  MENU_SAVED_STTY="$(stty -g 2>/dev/null)" || return 1
  trap 'restore_menu_terminal; exit 130' HUP INT TERM
  stty -echo -icanon min 1 time 0
  first="$(dd bs=1 count=1 2>/dev/null || true)"
  key=other
  case "$first" in
    "") key=enter ;;
    "$(printf '\033')")
      # Arrow keys continue with "[A"/"[B". A lone Escape (controller B)
      # must cancel instead of blocking while waiting for bytes that never come.
      stty min 0 time 1
      second="$(dd bs=1 count=1 2>/dev/null || true)"
      third="$(dd bs=1 count=1 2>/dev/null || true)"
      case "$second$third" in
        "[A"|"OA") key=up ;;
        "[B"|"OB") key=down ;;
        *) key=cancel ;;
      esac
      ;;
  esac
  restore_menu_terminal
  trap - HUP INT TERM
  printf '\n'
  MENU_KEY="$key"
}

choose_installed_action() {
  selected=restore
  while :; do
    say "is installed and selected as Main."
    echo "Use UP/DOWN to choose, A/Enter to continue, or B/Escape to cancel."
    if [ "$selected" = restore ]; then
      echo "> Restore stock MiSTer"
      echo "  Fully uninstall MiSTer MagiK"
    else
      echo "  Restore stock MiSTer"
      echo "> Fully uninstall MiSTer MagiK"
    fi
    if ! read_menu_key; then
      say "interactive input is unavailable; no changes made."
      return 1
    fi
    case "$MENU_KEY" in
      up|down)
        if [ "$selected" = restore ]; then selected=uninstall; else selected=restore; fi
        ;;
      enter) SELECTED_ACTION="$selected"; return 0 ;;
      cancel) say "cancelled; no changes made."; return 1 ;;
    esac
  done
}

confirm_install() {
  echo
  echo "MiSTer MagiK currently only supports 1080p."
  echo "Other resolutions and CRT support are coming later."
  echo
  echo "Continue by pressing A/Enter. Any other key cancels."
  if [ "${MISTER_MAGIK_TEST_MODE:-0}" = 1 ] && [ "${MISTER_MAGIK_TEST_CONFIRM_INSTALL:-0}" = 1 ]; then
    return 0
  fi
  if ! read_menu_key; then
    say "interactive input is unavailable; installation refused."
    return 1
  fi
  if [ "$MENU_KEY" != enter ]; then
    say "cancelled; no changes made."
    return 1
  fi
}

confirm_uninstall() {
  echo
  echo "WARNING: This permanently removes MiSTer MagiK, its settings, catalog,"
  echo "downloaded media, installer scripts, update_all entry, and saved backup."
  echo "Stock MiSTer boot will be restored first."
  echo
  echo "Press A/Enter to confirm. Press any other button to cancel."
  if [ "${MISTER_MAGIK_TEST_MODE:-0}" = 1 ] && [ "${MISTER_MAGIK_TEST_CONFIRM_UNINSTALL:-0}" = 1 ]; then
    return 0
  fi
  if ! read_menu_key; then
    say "interactive input is unavailable; uninstall refused."
    return 1
  fi
  if [ "$MENU_KEY" != enter ]; then
    say "cancelled; no changes made."
    return 1
  fi
}

sync_before_normal_reboot() {
  if [ "${MISTER_MAGIK_TEST_MODE:-0}" = 1 ]; then
    [ -z "${MISTER_MAGIK_TEST_REBOOT_TRACE:-}" ] || printf 'sync\n' >>"$MISTER_MAGIK_TEST_REBOOT_TRACE"
  else
    sync
  fi
}

request_normal_reboot() {
  if [ "${MISTER_MAGIK_TEST_MODE:-0}" = 1 ]; then
    [ -z "${MISTER_MAGIK_TEST_REBOOT_TRACE:-}" ] || printf 'reboot\n' >>"$MISTER_MAGIK_TEST_REBOOT_TRACE"
    say "TEST: normal reboot requested."
  else
    reboot
  fi
}

offer_normal_reboot() {
  echo
  echo "Reboot now? Press A/Enter to reboot. Any other key exits without rebooting."
  if ! read_menu_key; then
    say "interactive input is unavailable; reboot not requested."
    return 0
  fi
  if [ "$MENU_KEY" != enter ]; then
    say "reboot skipped."
    return 0
  fi

  say "syncing storage and rebooting normally."
  sync_before_normal_reboot
  request_normal_reboot
}

snapshot() {
  stamp="$(date +%Y%m%d-%H%M%S 2>/dev/null || echo unknown)"
  snap="$SNAP_DIR/$stamp-script"
  mkdir -p "$snap"
  cp "$INITTAB" "$snap/inittab" 2>/dev/null || true
  cp "$INI" "$snap/MiSTer.ini" 2>/dev/null || true
  ps > "$snap/ps.txt" 2>/dev/null || true
  cat /sys/module/MiSTer_fb/parameters/mode > "$snap/fb-mode.txt" 2>/dev/null || true
  cp /tmp/mister-magik-main.log "$snap/mister-magik-main.log" 2>/dev/null || true
  say "snapshot: $snap"
}

manifest_field() {
  key="$1"
  count="$(grep -c "^$key=" "$MANIFEST" 2>/dev/null || true)"
  [ "$count" = 1 ] || return 1
  sed -n "s/^$key=//p" "$MANIFEST"
}

installed_path() {
  case "$1" in
    /media/fat/*) printf '%s/%s\n' "$FAT" "${1#/media/fat/}" ;;
    *) return 1 ;;
  esac
}

verify_platform() {
  [ -r "$MANIFEST" ] || { say "ERROR: missing $MANIFEST."; return 1; }
  fields="$(awk -F= 'NF == 2 && $1 != "" && $2 != "" { count++ } END { print count + 0 }' "$MANIFEST")"
  records="$(awk 'NF && $0 !~ /^#/ { count++ } END { print count + 0 }' "$MANIFEST")"
  [ "$fields" = 19 ] && [ "$records" = 19 ] || { say "ERROR: platform manifest has unexpected fields."; return 1; }
  [ "$(manifest_field format)" = mister-magik-platform-v1 ] || return 1

  for name in main gui catalog_builder scanout_module scanout_metadata latch_rbf latch_metadata; do
    case "$name" in
      main) expected=/media/fat/MiSTer_MagiK ;;
      gui) expected=/media/fat/mister-magik/mister-magik-fb ;;
      catalog_builder) expected=/media/fat/mister-magik/mister-magik-catalog-builder ;;
      scanout_module) expected=/media/fat/mister-magik/mister_magik_scanout_slots.ko ;;
      scanout_metadata) expected=/media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt ;;
      latch_rbf) expected=/media/fat/mister-magik/fpga/menu-magik-vblank-latch.rbf ;;
      latch_metadata) expected=/media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt ;;
    esac
    device="$(manifest_field "${name}_path")" || return 1
    [ "$device" = "$expected" ] || { say "ERROR: invalid ${name}_path."; return 1; }
    file="$(installed_path "$device")" || return 1
    expected_hash="$(manifest_field "${name}_sha256")" || return 1
    [ -r "$file" ] || { say "ERROR: missing $file."; return 1; }
    actual_hash="$(sha256sum "$file" | awk '{print $1}')"
    [ "$actual_hash" = "$expected_hash" ] || { say "ERROR: hash mismatch for $file."; return 1; }
  done

  contract="$(manifest_field platform_contract_sha256)" || return 1
  menu_revision="$(manifest_field menu_revision)" || return 1
  main_revision="$(manifest_field main_revision)" || return 1
  magik_revision="$(manifest_field magik_revision)" || return 1
  for revision in "$main_revision" "$magik_revision" "$menu_revision"; do
    echo "$revision" | grep -Eq '^[0-9a-f]{40}$' || return 1
  done
  echo "$contract" | grep -Eq '^[0-9a-f]{64}$' || return 1
  module_hash="$(manifest_field scanout_module_sha256)" || return 1
  rbf_hash="$(manifest_field latch_rbf_sha256)" || return 1
  module_meta="$APP_DIR/mister_magik_scanout_slots.metadata.txt"
  rbf_meta="$APP_DIR/fpga/menu-magik-vblank-latch.metadata.txt"
  grep -qx "platform_contract_sha256=$contract" "$module_meta" || return 1
  grep -qx "platform_contract_sha256=$contract" "$rbf_meta" || return 1
  grep -qx "module_sha256=$module_hash" "$module_meta" || return 1
  grep -qx "rbf_sha256=$rbf_hash" "$rbf_meta" || return 1
  grep -qx "source_commit=$menu_revision" "$rbf_meta" || return 1
  case "$(sed -n 's/^vermagic=//p' "$module_meta")" in
    5.15.1-MiSTer\ *) ;;
    *) say "ERROR: scanout module vermagic is incompatible."; return 1 ;;
  esac
  say "verified platform $(manifest_field magik_revision)"
}

remove_legacy_root_legal_files() {
  legacy_failed=0
  rm -f \
    "$FAT/licenses/MiSTer-MagiK-GPL-3.0-or-later.txt" \
    "$FAT/licenses/RUST-LIBRARIES.txt" \
    "$FAT/licenses/FFMPEG-LGPL-2.1-or-later.txt" \
    "$FAT/licenses/PRESS-START-2P-OFL-1.1.txt" \
    "$FAT/THIRD-PARTY-NOTICES.txt" "$FAT/SOURCE-OFFER.txt" || legacy_failed=1
  rmdir "$FAT/licenses" 2>/dev/null || true
  [ "$legacy_failed" = 0 ]
}

ensure_stock_inittab() {
  if [ "${MISTER_MAGIK_TEST_MODE:-0}" != 1 ]; then mount -o remount,rw / 2>/dev/null || true; fi
  tmp=/tmp/inittab.magik
  awk '
BEGIN { wrote = 0 }
/^::sysinit:\/media\/fat\/MiSTer[[:space:]]*&/ {
  if (!wrote) {
    print "::sysinit:/media/fat/MiSTer &"
    wrote = 1
  }
  next
}
/^::sysinit:\/media\/fat\/MiSTer_MagiK/ { next }
/^::sysinit:\/media\/fat\/mister-magik\/boot\.sh/ { next }
{ print }
END {
  if (!wrote) print "::sysinit:/media/fat/MiSTer &"
}
' "$INITTAB" > "$tmp"
  cp "$tmp" "$INITTAB"
}

write_ini_with_selected_value() {
  selected_section="$1"
  selected_key="$2"
  selected_value="$3"
  if [ ! -f "$INI" ]; then
    printf '[%s]\n%s=%s\n' "$selected_section" "$selected_key" "$selected_value" > "$PENDING"
  else
    cr="$(awk 'sub(/\r$/, "") { print "\r"; exit }' "$INI")"
    awk -v selected_section="$selected_section" -v selected_key="$selected_key" -v selected_value="$selected_value" -v cr="$cr" '
BEGIN { in_selected = 0; saw_section = 0; wrote_value = 0 }
function is_selected_assignment(line, key) {
  if (line !~ /=/) return 0
  key = line
  sub(/=.*/, "", key)
  gsub(/[[:space:]]/, "", key)
  return tolower(key) == tolower(selected_key)
}
function write_value_if_needed() {
  if (in_selected && !wrote_value) {
    print selected_key "=" selected_value cr
    wrote_value = 1
  }
}
{ sub(/\r$/, "") }
/^[[:space:]]*\[[^]]+\]/ {
  write_value_if_needed()
  section = $0
  sub(/^[[:space:]]*\[/, "", section)
  sub(/\].*$/, "", section)
  gsub(/[[:space:]]/, "", section)
  in_selected = (tolower(section) == tolower(selected_section))
  if (in_selected) saw_section = 1
  print $0 cr
  next
}
in_selected && is_selected_assignment($0) {
  if (!wrote_value) {
    suffix = ""
    if (match($0, /[;#]/)) suffix = " " substr($0, RSTART)
    print selected_key "=" selected_value suffix cr
    wrote_value = 1
  } else {
    # Keep duplicate assignments as comments so canonicalization does not
    # discard user context while still leaving exactly one effective value.
    print ";" $0 cr
  }
  next
}
{ print $0 cr }
END {
  write_value_if_needed()
  if (!saw_section) {
    print cr
    print "[" selected_section "]" cr
    print selected_key "=" selected_value cr
  }
}
' "$INI" > "$PENDING"
  fi

  sync "$PENDING" 2>/dev/null || sync
  mv "$PENDING" "$INI"
  sync "$INI" 2>/dev/null || sync
}

write_ini_with_main() {
  mkdir -p "$APP_DIR"
  backup_ini_before_magik
  write_ini_with_selected_value MiSTer main MiSTer_MagiK
  write_ini_with_selected_value Menu video_mode 8
}

write_ini_with_stock_main() {
  write_ini_with_selected_value MiSTer main MiSTer
}

verify_stock_boot() {
  ini_has_one_main MiSTer || { say "ERROR: MiSTer.ini does not select exactly one stock Main."; return 1; }
  stock_count="$(grep -c '^::sysinit:/media/fat/MiSTer[[:space:]]*&[[:space:]]*$' "$INITTAB" 2>/dev/null || true)"
  [ "$stock_count" = 1 ] || { say "ERROR: inittab does not contain exactly one stock Main entry."; return 1; }
  if grep -Eq '^::sysinit:/media/fat/(MiSTer_MagiK|mister-magik/boot\.sh)' "$INITTAB" 2>/dev/null; then
    say "ERROR: inittab still contains a MagiK boot entry."
    return 1
  fi
}

restore_stock_boot() {
  snapshot
  ensure_stock_inittab
  write_ini_with_stock_main
  [ "${MISTER_MAGIK_TEST_MODE:-0}" = 1 ] || sync
  verify_stock_boot
}

install_magik() {
  confirm_install || return 1
  verify_platform || { say "ERROR: platform verification failed; boot configuration was not changed."; exit 1; }
  remove_legacy_root_legal_files
  snapshot
  chmod +x "$MAIN_BIN" "$GUI_BIN" "$CATALOG_BUILDER"
  ensure_stock_inittab
  write_ini_with_main
  [ "${MISTER_MAGIK_TEST_MODE:-0}" = 1 ] || sync
  say "installed. Reboot to start MiSTer MagiK."
  offer_normal_reboot
}

restore_magik() {
  restore_stock_boot
  say "stock MiSTer boot restored. MiSTer MagiK files were preserved."
  offer_normal_reboot
}

stop_magik_children() {
  [ "${MISTER_MAGIK_TEST_MODE:-0}" = 1 ] && return 0
  for process in mister-magik-fb mister-magik-catalog-builder; do
    pids="$(pidof "$process" 2>/dev/null || true)"
    [ -z "$pids" ] || kill $pids 2>/dev/null || true
  done
  sleep 1
  for process in mister-magik-fb mister-magik-catalog-builder; do
    pids="$(pidof "$process" 2>/dev/null || true)"
    [ -z "$pids" ] || kill -9 $pids 2>/dev/null || true
  done
}

remove_owned_files() {
  cleanup_failed=0
  rm -rf "$APP_DIR" || cleanup_failed=1
  rm -f "$MAIN_BIN" "$CHANNEL_SCRIPT" "$DOWNLOADER_DROP_IN" || cleanup_failed=1
  rm -f "$FAT"/downloader_mister_magik.ini.tmp.* "$FAT"/.downloader_mister_magik.ini* || cleanup_failed=1
  rm -f "$INI_BACKUP" "$FAT"/.MiSTer.ini.bak.before-magik.new.* "$FAT"/.MiSTer.ini.magik.new* || cleanup_failed=1
  remove_legacy_root_legal_files || cleanup_failed=1

  # The installed copy removes itself only after every other owned path.
  rm -f "$INSTALLED_SCRIPT" || cleanup_failed=1
  [ "${MISTER_MAGIK_TEST_MODE:-0}" = 1 ] || sync

  for path in "$APP_DIR" "$MAIN_BIN" "$INSTALLED_SCRIPT" "$CHANNEL_SCRIPT" \
    "$DOWNLOADER_DROP_IN" "$INI_BACKUP" "$FAT/THIRD-PARTY-NOTICES.txt" \
    "$FAT/SOURCE-OFFER.txt"; do
    if [ -e "$path" ]; then say "ERROR: uninstall residue: $path"; cleanup_failed=1; fi
  done
  for path in \
    "$FAT/licenses/MiSTer-MagiK-GPL-3.0-or-later.txt" \
    "$FAT/licenses/RUST-LIBRARIES.txt" \
    "$FAT/licenses/FFMPEG-LGPL-2.1-or-later.txt" \
    "$FAT/licenses/PRESS-START-2P-OFL-1.1.txt" \
    "$FAT"/downloader_mister_magik.ini.tmp.* "$FAT"/.downloader_mister_magik.ini* \
    "$FAT"/.MiSTer.ini.bak.before-magik.new.* "$FAT"/.MiSTer.ini.magik.new*; do
    if [ -e "$path" ]; then say "ERROR: uninstall residue: $path"; cleanup_failed=1; fi
  done
  [ "$cleanup_failed" = 0 ]
}

uninstall_magik() {
  confirm_uninstall || return 1
  restore_stock_boot
  stop_magik_children
  if ! remove_owned_files; then
    say "ERROR: uninstall incomplete; review the residue above. Stock boot is restored."
    return 1
  fi
  say "fully uninstalled."
  offer_normal_reboot
}

action="${1:-}"
if [ -z "$action" ] && ini_selects_magik; then
  SELECTED_ACTION=""
  if ! choose_installed_action; then
    pause_exit
    exit 0
  fi
  action="$SELECTED_ACTION"
fi

case "$action" in
  install|"")
    install_magik
    ;;
  restore)
    restore_magik
    ;;
  uninstall)
    uninstall_magik
    ;;
  *)
    echo "Usage: $0 [install|restore|uninstall]"
    exit 2
    ;;
esac

pause_exit
