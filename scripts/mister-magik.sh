#!/bin/sh
# MiSTer MagiK installer for MiSTer Scripts menu / Downloader / update_all.

set -eu

FAT="${MISTER_MAGIK_FAT:-/media/fat}"
INITTAB="${MISTER_MAGIK_INITTAB:-/etc/inittab}"
INI="$FAT/MiSTer.ini"
APP_DIR="$FAT/mister-magik"
MAIN_BIN="$FAT/MiSTer_MagiK"
GUI_BIN="$APP_DIR/mister-magik-fb"
CATALOG_BUILDER="$APP_DIR/mister-magik-catalog-builder"
SLINT_LOGO_FILE="$APP_DIR/art/slint-logo-pixel.rgba"
SNAP_DIR="$APP_DIR/snapshots"
PENDING="$FAT/.MiSTer.ini.magik.new"
MANIFEST="$APP_DIR/platform-v1.manifest"

say() {
  echo "MiSTer MagiK: $*"
}

pause_exit() {
  [ "${MISTER_MAGIK_NO_PAUSE:-0}" = 1 ] && return 0
  echo
  echo "Press Enter to exit."
  read _ || true
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
  if [ ! -f "$SLINT_LOGO_FILE" ]; then
    say "ERROR: $SLINT_LOGO_FILE is missing."
    return 1
  fi
  say "verified platform $(manifest_field magik_revision)"
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

write_ini_with_main() {
  mkdir -p "$APP_DIR"
  if [ ! -f "$INI" ]; then
    printf '[MiSTer]\nmain=MiSTer_MagiK\n' > "$PENDING"
  else
    cp -n "$INI" "$INI.bak" 2>/dev/null || true
    awk '
BEGIN { in_mister = 0; saw_mister = 0; wrote_main = 0 }
function write_main_if_needed() {
  if (in_mister && !wrote_main) {
    print "main=MiSTer_MagiK"
    wrote_main = 1
  }
}
/^[[:space:]]*\[[^]]+\]/ {
  write_main_if_needed()
  section = $0
  sub(/^[[:space:]]*\[/, "", section)
  sub(/\].*$/, "", section)
  gsub(/[[:space:]]/, "", section)
  in_mister = (tolower(section) == "mister")
  if (in_mister) saw_mister = 1
  print
  next
}
in_mister && /^[[:space:]]*main[[:space:]]*=/ {
  if (!wrote_main) {
    print "main=MiSTer_MagiK"
    wrote_main = 1
  }
  next
}
{ print }
END {
  write_main_if_needed()
  if (!saw_mister) {
    print ""
    print "[MiSTer]"
    print "main=MiSTer_MagiK"
  }
}
' "$INI" > "$PENDING"
  fi

  sync "$PENDING" 2>/dev/null || sync
  mv "$PENDING" "$INI"
  sync "$INI" 2>/dev/null || sync
}

remove_ini_main() {
  [ -f "$INI" ] || return 0
  cp -n "$INI" "$INI.bak" 2>/dev/null || true
  awk '
BEGIN { in_mister = 0 }
/^[[:space:]]*\[[^]]+\]/ {
  section = $0
  sub(/^[[:space:]]*\[/, "", section)
  sub(/\].*$/, "", section)
  gsub(/[[:space:]]/, "", section)
  in_mister = (tolower(section) == "mister")
  print
  next
}
in_mister && /^[[:space:]]*main[[:space:]]*=[[:space:]]*MiSTer_MagiK[[:space:]]*([;#].*)?$/ { next }
{ print }
' "$INI" > "$PENDING"
  sync "$PENDING" 2>/dev/null || sync
  mv "$PENDING" "$INI"
  sync "$INI" 2>/dev/null || sync
}

install_magik() {
  verify_platform || { say "ERROR: platform verification failed; boot configuration was not changed."; exit 1; }
  snapshot
  chmod +x "$MAIN_BIN" "$GUI_BIN" "$CATALOG_BUILDER"
  ensure_stock_inittab
  write_ini_with_main
  [ "${MISTER_MAGIK_TEST_MODE:-0}" = 1 ] || sync
  say "installed. Reboot to start MiSTer MagiK."
}

disable_magik() {
  snapshot
  ensure_stock_inittab
  remove_ini_main
  [ "${MISTER_MAGIK_TEST_MODE:-0}" = 1 ] || sync
  say "disabled. Reboot to return to stock MiSTer boot."
}

case "${1:-}" in
  install|enable|"")
    install_magik
    ;;
  disable|uninstall)
    disable_magik
    ;;
  *)
    echo "Usage: $0 [install|disable]"
    exit 2
    ;;
esac

pause_exit
