#!/usr/bin/env bash
# Build and deploy the Main-as-parent experiment:
#   - magik-gui Slint binary -> /media/fat/mister-magic/mister-magic-fb
#   - main-mister fork       -> /media/fat/MiSTer_Magic
#   - MiSTer.ini main=MiSTer_Magic
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUI_DIR="$ROOT/magik-gui"
MAIN_DIR="$ROOT/main-mister"
GUI_REMOTE="/media/fat/mister-magic/mister-magic-fb"
MAIN_REMOTE="/media/fat/MiSTer_Magic"
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
  make -C "$MAIN_DIR"
else
  "$MAIN_DIR/build-docker.sh"
fi

echo "==> Deploying experiment binaries"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$ROOT/scripts/mister_ssh.py" run "mkdir -p /media/fat/mister-magic"

MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$ROOT/scripts/mister_ssh.py" put "$GUI_BIN" "$GUI_REMOTE"

MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$ROOT/scripts/mister_ssh.py" put "$MAIN_BIN" "$MAIN_REMOTE"

MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$ROOT/scripts/mister_ssh.py" run "chmod +x '$GUI_REMOTE' '$MAIN_REMOTE'"

echo "==> Enabling main=MiSTer_Magic"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$ROOT/scripts/mister_ssh.py" run "cp -n /media/fat/MiSTer.ini /media/fat/MiSTer.ini.before-mister-magic-main 2>/dev/null || true; grep -v '^main=' /media/fat/MiSTer.ini > /tmp/MiSTer.ini.magic; printf '%s\n' 'main=MiSTer_Magic' >> /tmp/MiSTer.ini.magic; cp /tmp/MiSTer.ini.magic /media/fat/MiSTer.ini; sync"

echo "==> Installed. Reboot to start MiSTer_Magic."
echo "    Restore stock by removing main=MiSTer_Magic or running scripts/restore-stock-boot.sh if Slint boot is installed."
