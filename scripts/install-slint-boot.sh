#!/usr/bin/env bash
# Install update_all-compatible Magik boot through MiSTer's native main= hook.
#
# Stock /media/fat/MiSTer stays as the only inittab menu entry. It reads
# MiSTer.ini and re-execs /media/fat/MiSTer_MagiK, matching Zaparoo's model.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

MISTER_IP="${MISTER_IP:?Set MISTER_IP}"
MISTER_PASS="${MISTER_PASS:-1}"

echo "==> Configure device (stock inittab + MiSTer.ini main=MiSTer_MagiK)"
MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" uv run python scripts/mister_ssh.py run '
set -e
if [ ! -x /media/fat/MiSTer_MagiK ]; then
  echo "ERROR: /media/fat/MiSTer_MagiK is missing or not executable"
  echo "Run scripts/deploy-main-mister-experiment.sh first."
  exit 1
fi

INI=/media/fat/MiSTer.ini
STAMP=$(date +%Y%m%d-%H%M%S 2>/dev/null || echo unknown)
SNAP="/media/fat/mister-magik/snapshots/$STAMP-install"
mkdir -p "$SNAP"
cp /etc/inittab "$SNAP/inittab" 2>/dev/null || true
cp "$INI" "$SNAP/MiSTer.ini" 2>/dev/null || true
ps > "$SNAP/ps.txt" 2>/dev/null || true
cat /sys/module/MiSTer_fb/parameters/mode > "$SNAP/fb-mode.txt" 2>/dev/null || true
cp /tmp/mister-magik-main.log "$SNAP/mister-magik-main.log" 2>/dev/null || true
echo "snapshot: $SNAP"
if [ ! -f "$INI.before-mister-magik-main" ]; then cp "$INI" "$INI.before-mister-magik-main"; fi

# Set the native main= handoff to the fork in [MiSTer] only. Leave unrelated
# per-core main= entries alone.
tmp="$INI.new"
awk '"'"'
BEGIN { in_mister = 0; seen_mister = 0; wrote = 0 }
/^\[[^]]+\]/ {
  if (in_mister && !wrote) {
    print "main=MiSTer_MagiK"
    wrote = 1
  }
  in_mister = 0
  if (tolower($0) == "[mister]") {
    in_mister = 1
    seen_mister = 1
    wrote = 0
  }
}
in_mister && tolower($0) ~ /^[[:space:]]*main[[:space:]]*=/ {
  if (!wrote) {
    print "main=MiSTer_MagiK"
    wrote = 1
  }
  next
}
!in_mister && tolower($0) ~ /^[[:space:]]*main[[:space:]]*=[[:space:]]*mister_magik[[:space:]]*$/ {
  next
}
{ print }
END {
  if (in_mister && !wrote) print "main=MiSTer_MagiK"
  if (!seen_mister) {
    print "[MiSTer]"
    print "main=MiSTer_MagiK"
  }
}
'"'"' "$INI" > "$tmp"
mv "$tmp" "$INI"
echo "main=MiSTer_MagiK ensured"

# Force the menu core to 1080p. MiSTer_MagiK still loads the MENU core, and the
# fork also treats Magik-named sections as menu aliases.
tmp="$INI.new"
awk '"'"'
BEGIN { in_menu = 0; seen_menu = 0; wrote = 0 }
/^\[[^]]+\]/ {
  if (in_menu && !wrote) {
    print "video_mode=8"
    wrote = 1
  }
  in_menu = 0
  if (tolower($0) == "[menu]") {
    in_menu = 1
    seen_menu = 1
    wrote = 0
  }
}
in_menu && tolower($0) ~ /^[[:space:]]*video_mode[[:space:]]*=/ {
  if (!wrote) {
    print "video_mode=8"
    wrote = 1
  }
  next
}
{ print }
END {
  if (in_menu && !wrote) print "video_mode=8"
  if (!seen_menu) {
    print ""
    print "[Menu]"
    print "video_mode=8"
  }
}
'"'"' "$INI" > "$tmp"
mv "$tmp" "$INI"
echo "Menu video_mode=8 ensured"

# Current Main_MiSTer release accepts vrr_mode and vrr_vesa_framerate, but not
# these min/max keys. Leave them documented, never active, to avoid boot warnings.
for ini_path in "$INI" /media/fat/MiSTer.ini.bak; do
  [ -f "$ini_path" ] || continue
  tmp="$ini_path.new"
  awk '"'"'
tolower($0) ~ /^[[:space:]]*vrr_(min|max)_framerate[[:space:]]*=/ {
  print ";" $0
  next
}
{ print }
'"'"' "$ini_path" > "$tmp"
  mv "$tmp" "$ini_path"
done
echo "unsupported VRR min/max keys commented"

# Stock MiSTer starts first, then MiSTer.ini main= hands off to MiSTer_MagiK.
mount -o remount,rw / 2>/dev/null || true
tmp=/tmp/inittab.magik
awk '"'"'
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
'"'"' /etc/inittab > "$tmp"
cp "$tmp" /etc/inittab
echo "inittab ensured -> stock MiSTer"
sync

echo "=== post-install inittab ==="
grep -n "sysinit" /etc/inittab | grep -E "MiSTer|MagiK|boot.sh" || true
echo "=== post-install MiSTer.ini boot keys ==="
awk '"'"'BEGIN{s="global"} /^\[/ {s=$0} /^[[:space:]]*(main|video_mode|direct_video)[[:space:]]*=/ {print s " " NR ":" $0}'"'"' "$INI"
echo "=== post-install processes ==="
ps | grep -E "[M]iSTer|[M]iSTer_MagiK|[m]ister-magik-fb" || true
echo "=== post-install fb mode ==="
cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true
'

echo "==> Reboot to apply"
MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" uv run python scripts/mister_ssh.py reboot-wait

echo "Done. Stock MiSTer should hand off to MiSTer_MagiK via MiSTer.ini main=."
