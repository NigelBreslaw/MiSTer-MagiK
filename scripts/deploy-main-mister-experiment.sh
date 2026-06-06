#!/usr/bin/env bash
# Build and deploy the Main-as-parent experiment:
#   - magik-gui Slint binary -> /media/fat/mister-magik/mister-magik-fb
#   - main-mister fork       -> /media/fat/MiSTer_MagiK
#   - boot config            -> stock inittab + MiSTer.ini main=MiSTer_MagiK
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUI_DIR="$ROOT/magik-gui"
MAIN_DIR="$ROOT/main-mister"
GUI_REMOTE="/media/fat/mister-magik/mister-magik-fb"
MAIN_REMOTE="/media/fat/MiSTer_MagiK"
GUI_PROFILE=release
GUI_BUILD_ARGS=(--fast)

for arg in "$@"; do
  case "$arg" in
    --device)
      GUI_PROFILE=release-device
      GUI_BUILD_ARGS=(--device)
      ;;
    --fast)
      GUI_PROFILE=release
      GUI_BUILD_ARGS=(--fast)
      ;;
    -h|--help)
      sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
  esac
done

GUI_BIN="$GUI_DIR/target/armv7-unknown-linux-gnueabihf/$GUI_PROFILE/mister-magik-fb"
MAIN_BIN="$MAIN_DIR/bin/MiSTer"

echo "==> Building magik-gui ($GUI_PROFILE)"
"$GUI_DIR/build-arm.sh" "${GUI_BUILD_ARGS[@]}"

echo "==> Building main-mister"
if command -v arm-none-linux-gnueabihf-gcc >/dev/null 2>&1; then
  make -C "$MAIN_DIR" clean
  make -C "$MAIN_DIR"
else
  "$MAIN_DIR/build-docker.sh" clean
  "$MAIN_DIR/build-docker.sh"
fi

echo "==> Deploying experiment binaries"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$ROOT/scripts/mister_ssh.py" run '
set -e
STAMP=$(date +%Y%m%d-%H%M%S 2>/dev/null || echo unknown)
SNAP="/media/fat/mister-magik/snapshots/$STAMP-deploy"
mkdir -p "$SNAP" /media/fat/mister-magik
cp /etc/inittab "$SNAP/inittab" 2>/dev/null || true
cp /media/fat/MiSTer.ini "$SNAP/MiSTer.ini" 2>/dev/null || true
ps > "$SNAP/ps.txt" 2>/dev/null || true
cat /sys/module/MiSTer_fb/parameters/mode > "$SNAP/fb-mode.txt" 2>/dev/null || true
cp /tmp/mister-magik-main.log "$SNAP/mister-magik-main.log" 2>/dev/null || true
echo "snapshot: $SNAP"
kill -9 $(pidof mister-magik-fb) 2>/dev/null || true
kill -9 $(pidof MiSTer_MagiK) 2>/dev/null || true
'

MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$ROOT/scripts/mister_ssh.py" put "$GUI_BIN" "$GUI_REMOTE.upload"

MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$ROOT/scripts/mister_ssh.py" put "$MAIN_BIN" "$MAIN_REMOTE.upload"

MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$ROOT/scripts/mister_ssh.py" run "mv '$GUI_REMOTE.upload' '$GUI_REMOTE'; mv '$MAIN_REMOTE.upload' '$MAIN_REMOTE'; chmod +x '$GUI_REMOTE' '$MAIN_REMOTE'"

echo "==> Enabling stock inittab + MiSTer.ini main=MiSTer_MagiK boot"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$ROOT/scripts/mister_ssh.py" run '
set -e
mount -o remount,rw / 2>/dev/null || true
INI=/media/fat/MiSTer.ini
cp -n "$INI" "$INI.before-mister-magik-main" 2>/dev/null || true

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

echo "==> Installed. Reboot to let stock MiSTer hand off to MiSTer_MagiK via MiSTer.ini main=."
echo "    Restore stock with scripts/restore-stock-boot.sh."
