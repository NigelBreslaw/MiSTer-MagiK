#!/usr/bin/env bash
# Build and deploy the Main-as-parent experiment:
#   - magik-gui Slint binary -> /media/fat/mister-magic/mister-magic-fb
#   - main-mister fork       -> /media/fat/MiSTer_Magik
#   - inittab boots          -> /media/fat/MiSTer_Magik
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUI_DIR="$ROOT/magik-gui"
MAIN_DIR="$ROOT/main-mister"
GUI_REMOTE="/media/fat/mister-magic/mister-magic-fb"
MAIN_REMOTE="/media/fat/MiSTer_Magik"
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

GUI_BIN="$GUI_DIR/target/armv7-unknown-linux-gnueabihf/$GUI_PROFILE/mister-magic-fb"
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
  uv run python "$ROOT/scripts/mister_ssh.py" run "kill -9 \$(pidof mister-magic-fb) 2>/dev/null || true; kill -9 \$(pidof MiSTer_Magik) 2>/dev/null || true; mkdir -p /media/fat/mister-magic"

MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$ROOT/scripts/mister_ssh.py" put "$GUI_BIN" "$GUI_REMOTE"

MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$ROOT/scripts/mister_ssh.py" put "$MAIN_BIN" "$MAIN_REMOTE"

MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$ROOT/scripts/mister_ssh.py" run "chmod +x '$GUI_REMOTE' '$MAIN_REMOTE'"

echo "==> Enabling direct MiSTer_Magik boot"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$ROOT/scripts/mister_ssh.py" run "mount -o remount,rw / 2>/dev/null || true; cp -n /media/fat/MiSTer.ini /media/fat/MiSTer.ini.before-mister-magik-main 2>/dev/null || true; grep -v '^main=' /media/fat/MiSTer.ini > /tmp/MiSTer.ini.magik; cp /tmp/MiSTer.ini.magik /media/fat/MiSTer.ini; if grep -q '^::sysinit:/media/fat/MiSTer_Magik ' /etc/inittab; then echo 'inittab already uses MiSTer_Magik'; elif grep -q '^::sysinit:/media/fat/MiSTer ' /etc/inittab; then sed -i 's|^::sysinit:/media/fat/MiSTer &|::sysinit:/media/fat/MiSTer_Magik \\&|' /etc/inittab; else sed -i 's|^::sysinit:/media/fat/mister-magic/boot.sh .*|::sysinit:/media/fat/MiSTer_Magik \\&|' /etc/inittab; fi; grep sysinit /etc/inittab | grep -E 'MiSTer|boot.sh'; sync"

echo "==> Installed. Reboot to start MiSTer_Magik."
echo "    Restore stock with scripts/restore-stock-boot.sh."
