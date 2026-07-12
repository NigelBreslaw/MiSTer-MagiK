#!/bin/sh
# MiSTer MagiK installer for MiSTer Scripts menu / Downloader / update_all.

set -eu

FAT=/media/fat
INI="$FAT/MiSTer.ini"
APP_DIR="$FAT/mister-magik"
MAIN_BIN="$FAT/MiSTer_MagiK"
GUI_BIN="$APP_DIR/mister-magik-fb"
ART_FILE="$APP_DIR/art/arcade-cabinet-preview.rgba"
SLINT_LOGO_FILE="$APP_DIR/art/slint-logo-pixel.rgba"
SNAP_DIR="$APP_DIR/snapshots"
PENDING="$FAT/.MiSTer.ini.magik.new"

say() {
  echo "MiSTer MagiK: $*"
}

pause_exit() {
  echo
  echo "Press Enter to exit."
  read _ || true
}

snapshot() {
  stamp="$(date +%Y%m%d-%H%M%S 2>/dev/null || echo unknown)"
  snap="$SNAP_DIR/$stamp-script"
  mkdir -p "$snap"
  cp /etc/inittab "$snap/inittab" 2>/dev/null || true
  cp "$INI" "$snap/MiSTer.ini" 2>/dev/null || true
  ps > "$snap/ps.txt" 2>/dev/null || true
  cat /sys/module/MiSTer_fb/parameters/mode > "$snap/fb-mode.txt" 2>/dev/null || true
  cp /tmp/mister-magik-main.log "$snap/mister-magik-main.log" 2>/dev/null || true
  say "snapshot: $snap"
}

ensure_files() {
  if [ ! -x "$MAIN_BIN" ]; then
    say "ERROR: $MAIN_BIN is missing or not executable."
    exit 1
  fi
  if [ ! -x "$GUI_BIN" ]; then
    say "ERROR: $GUI_BIN is missing or not executable."
    exit 1
  fi
  if [ ! -f "$ART_FILE" ]; then
    say "ERROR: $ART_FILE is missing."
    exit 1
  fi
  if [ ! -f "$SLINT_LOGO_FILE" ]; then
    say "ERROR: $SLINT_LOGO_FILE is missing."
    exit 1
  fi
}

ensure_stock_inittab() {
  mount -o remount,rw / 2>/dev/null || true
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
' /etc/inittab > "$tmp"
  cp "$tmp" /etc/inittab
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
  ensure_files
  snapshot
  chmod +x "$MAIN_BIN" "$GUI_BIN"
  ensure_stock_inittab
  write_ini_with_main
  sync
  say "installed. Reboot to start MiSTer MagiK."
}

disable_magik() {
  snapshot
  ensure_stock_inittab
  remove_ini_main
  sync
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
