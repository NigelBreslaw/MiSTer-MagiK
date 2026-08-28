#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/build/alpha-release}"
BIN="$ROOT/apps/mister/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"

if [[ "${2:-}" != "--skip-build" ]]; then
  "$ROOT/scripts/magik-ci" build runtime-device
fi

python3 "$ROOT/scripts/release/packaging/generate-third-party-licenses.py"
test -x "$BIN"
if [[ "$(cat "$BIN.features" 2>/dev/null || true)" != "ui" ]]; then
  echo "error: $BIN is not the required video-capable UI release binary" >&2
  exit 1
fi

mkdir -p "$OUT/licenses"
cp "$BIN" "$OUT/mister-magik-fb"
cp "$ROOT/LICENSE" "$OUT/LICENSE"
cp "$ROOT/apps/mister/licenses/FFMPEG.txt" "$OUT/licenses/"
cp "$ROOT/apps/mister/licenses/PRESS-START-2P.txt" "$OUT/licenses/"
cp "$ROOT/apps/mister/licenses/RUST-LIBRARIES.txt" "$OUT/licenses/"
cp "$ROOT/crates/particles/assets/cabinet/arcade-cabinet.LICENSE.txt" \
  "$OUT/licenses/ARCADE-CABINET-CC-BY-NC-4.0.txt"
cat > "$OUT/THIRD-PARTY-NOTICES.txt" <<'EOF'
MiSTer MagiK alpha distribution notices
========================================

Copyright (C) 2026 Nigel Breslaw

MiSTer MagiK is GPL-3.0-or-later. This directory contains the full GPL text,
plus the FFmpeg LGPL-2.1-or-later, Press Start 2P SIL OFL-1.1, Arcade Cabinet
CC-BY-NC-4.0 attribution, and generated normal-runtime Rust dependency notices
in licenses/.

The launcher uses Slint under its GPL-3.0-only option.
EOF
cat > "$OUT/SOURCE-OFFER.txt" <<EOF
Corresponding source and relinking instructions
===============================================

MiSTer MagiK source (including build and installation scripts):
  https://github.com/NigelBreslaw/MiSTer-MagiK/tree/$(git -C "$ROOT" rev-parse HEAD)

FFmpeg 8.1.2 source, used by this UI build:
  https://github.com/FFmpeg/FFmpeg/tree/n8.1.2
The exact configure flags and cross-build procedure are in
agent-cli/src/build.rs at the source revision above. The
MiSTer MagiK source, Cargo.lock, and build scripts are the complete source
needed to rebuild the application and relink it with a modified FFmpeg build.
EOF

echo "alpha release staged: $OUT"
