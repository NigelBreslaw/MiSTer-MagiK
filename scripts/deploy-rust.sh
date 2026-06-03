#!/usr/bin/env bash
# Cross-build the Rust frontend and deploy the binary to the MiSTer.
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh
#
# Installs to /media/fat/mister-magic/mister-magic-fb on the SD card.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$HERE/rust/target/armv7-unknown-linux-gnueabihf/release/mister-magic-fb"
REMOTE="/media/fat/mister-magic/mister-magic-fb"

echo "==> Cross-building (armv7 release)"
"$HERE/rust/build-arm.sh"

echo "==> Deploying $BIN -> $REMOTE"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$HERE/scripts/mister_ssh.py" run "mkdir -p /media/fat/mister-magic"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$HERE/scripts/mister_ssh.py" put "$BIN" "$REMOTE"
MISTER_IP="${MISTER_IP:-192.168.1.117}" \
MISTER_PASS="${MISTER_PASS:-1}" \
  uv run python "$HERE/scripts/mister_ssh.py" run "chmod +x $REMOTE"

echo "==> Deployed. Run on device (with menu SIGSTOPped so we own the SPI bus):"
echo "    MP=\$(pidof MiSTer); kill -STOP \$MP"
echo "    $REMOTE ui demo 20"
echo "    $REMOTE scenes   # … list_scroll (std-widgets ListView)"
echo "    kill -CONT \$MP"
