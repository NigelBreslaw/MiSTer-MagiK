#!/usr/bin/env bash
# Cross-build the Rust frontend and deploy the binary to the MiSTer.
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh
#   MISTER_IP=... scripts/deploy-rust.sh --fast    # thin LTO, quicker build
#
# Default installs the release-device (A3) binary — use --fast for daily iteration.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REMOTE="/media/fat/mister-magic/mister-magic-fb"

PROFILE=release-device
BUILD_FLAG=(--device)
for arg in "$@"; do
  case "$arg" in
    --fast) PROFILE=release; BUILD_FLAG=(--fast) ;;
    --device) PROFILE=release-device; BUILD_FLAG=(--device) ;;
    -h|--help)
      sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
  esac
done

BIN="$HERE/rust/target/armv7-unknown-linux-gnueabihf/$PROFILE/mister-magic-fb"

echo "==> Cross-building (armv7 profile=$PROFILE)"
"$HERE/rust/build-arm.sh" "${BUILD_FLAG[@]}"

echo "==> Deploying $BIN -> $REMOTE"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$HERE/scripts/mister_ssh.py" run "mkdir -p /media/fat/mister-magic"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$HERE/scripts/mister_ssh.py" put "$BIN" "$REMOTE"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$HERE/scripts/mister_ssh.py" put "$HERE/scripts/mister-magic/boot.sh" "/media/fat/mister-magic/boot.sh"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$HERE/scripts/mister_ssh.py" run "chmod +x $REMOTE /media/fat/mister-magic/boot.sh"

echo "==> Deployed ($PROFILE)."
echo "    Production boot: scripts/install-slint-boot.sh  (once — inittab handoff)"
echo "    Dev / bench:     kill -9 \$(pidof MiSTer); $REMOTE ui launcher 60"
echo "    Restore stock:   scripts/restore-stock-boot.sh"
