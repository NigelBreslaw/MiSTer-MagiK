#!/usr/bin/env bash
# Grab the MiSTer's framebuffer and save it as a PNG on this machine.
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/capture-fb.sh [out.png]
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${1:-$HERE/build/mister-fb.png}"
RAW="$HERE/build/mister-fb.raw"
W="${FB_W:-1920}"
H="${FB_H:-1080}"
REMOTE_BIN="${MISTER_MAGIC_BIN:-/media/fat/mister-magic/mister-magic-fb}"
REMOTE_PNG="/tmp/fb0.png"
REMOTE_RAW="/tmp/fb0.raw"
mkdir -p "$HERE/build"

export MISTER_IP="${MISTER_IP:-192.168.1.117}"
export MISTER_PASS="${MISTER_PASS:-1}"

if uv run python "$HERE/scripts/mister_ssh.py" run "$REMOTE_BIN capture-png $REMOTE_PNG $W $H"; then
  uv run python "$HERE/scripts/mister_ssh.py" get "$REMOTE_PNG" "$OUT"
  echo "Captured MiSTer framebuffer PNG -> $OUT"
else
  echo "Rust capture-png failed; falling back to raw dd + host conversion" >&2
  uv run python "$HERE/scripts/mister_ssh.py" run "dd if=/dev/fb0 of=$REMOTE_RAW bs=1M 2>/dev/null"
  uv run python "$HERE/scripts/mister_ssh.py" get "$REMOTE_RAW" "$RAW"
  python3 "$HERE/scripts/raw_to_png.py" "$RAW" "$W" "$H" "$OUT"
  echo "Captured MiSTer framebuffer -> $OUT"
fi
