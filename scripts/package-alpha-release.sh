#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/build/alpha-release}"
BIN="$ROOT/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"

if [[ "${2:-}" != "--skip-build" ]]; then
  "$ROOT/magik-gui/build-arm.sh" --video
fi

python3 "$ROOT/scripts/generate-third-party-licenses.py"
test -x "$BIN"
if [[ "$(cat "$BIN.features" 2>/dev/null || true)" != "ui,video" ]]; then
  echo "error: $BIN is not the required ui,video release binary" >&2
  exit 1
fi

mkdir -p "$OUT/licenses"
cp "$BIN" "$OUT/mister-magik-fb"
cp "$ROOT/LICENSE" "$OUT/LICENSE"
cp "$ROOT/magik-gui/licenses/FFMPEG.txt" "$OUT/licenses/"
cp "$ROOT/magik-gui/licenses/PRESS-START-2P.txt" "$OUT/licenses/"
cp "$ROOT/magik-gui/licenses/RUST-LIBRARIES.txt" "$OUT/licenses/"

echo "alpha release staged: $OUT"
