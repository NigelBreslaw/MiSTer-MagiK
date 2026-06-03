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
mkdir -p "$HERE/build"

export MISTER_IP="${MISTER_IP:-192.168.1.117}"
export MISTER_PASS="${MISTER_PASS:-1}"

uv run python "$HERE/scripts/mister_ssh.py" run "dd if=/dev/fb0 of=/tmp/fb0.raw bs=1M 2>/dev/null"
uv run python "$HERE/scripts/mister_ssh.py" get /tmp/fb0.raw "$RAW"
python3 "$HERE/scripts/raw_to_png.py" "$RAW" "$W" "$H" "$OUT"
echo "Captured MiSTer framebuffer -> $OUT"
