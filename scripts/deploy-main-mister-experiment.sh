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
GUI_PROFILE=release-device
GUI_BUILD_ARGS=(--device)

for arg in "$@"; do
  case "$arg" in
    --device)
      GUI_PROFILE=release-device
      GUI_BUILD_ARGS=(--device)
      ;;
    -h|--help)
      sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "ERROR: unknown argument: $arg" >&2
      exit 2
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
"$ROOT/scripts/mister" run '
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

"$ROOT/scripts/mister" put "$GUI_BIN" "$GUI_REMOTE.upload"

"$ROOT/scripts/mister" put "$MAIN_BIN" "$MAIN_REMOTE.upload"

"$ROOT/scripts/mister" run "mv '$GUI_REMOTE.upload' '$GUI_REMOTE'; mv '$MAIN_REMOTE.upload' '$MAIN_REMOTE'; chmod +x '$GUI_REMOTE' '$MAIN_REMOTE'"

echo "==> Enabling stock inittab + MiSTer.ini main=MiSTer_MagiK boot"
"$ROOT/scripts/mister" run '
set -e
mount -o remount,rw / 2>/dev/null || true
INI=/media/fat/MiSTer.ini
cp -n "$INI" "$INI.bak" 2>/dev/null || true

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
echo "=== post-install processes ==="
ps | grep -E "[M]iSTer|[M]iSTer_MagiK|[m]ister-magik-fb" || true
echo "=== post-install fb mode ==="
cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true
'

"$ROOT/scripts/mister" ini-repair-boot
"$ROOT/scripts/mister" ini-repair-arcade-video

echo "==> Installed. Reboot to let stock MiSTer hand off to MiSTer_MagiK via MiSTer.ini main=."
echo "    Restore stock with scripts/restore-stock-boot.sh."
