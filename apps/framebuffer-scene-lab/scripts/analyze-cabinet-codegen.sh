#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(cd "$APP_DIR/../.." && pwd)"
BIN="${1:-$APP_DIR/target/armv7-unknown-linux-gnueabihf/release-device-profile/mister-magik-framebuffer-scene-lab}"
REVISION="$(git -C "$ROOT" rev-parse HEAD)"
OUT_DIR="$ROOT/build/cabinet-codegen/$REVISION"
APPLE_IMAGE="${MISTER_APPLE_CONTAINER_IMAGE:-mister-magik-cross-armv7:ubuntu20-arm64}"

if [[ ! -f "$BIN" || "$BIN" != "$ROOT/"* ]]; then
  echo "cabinet analysis requires a linked scene-lab binary under $ROOT: $BIN" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
REL_BIN="/project/${BIN#"$ROOT/"}"
REL_OUT="/project/${OUT_DIR#"$ROOT/"}"

if [[ "$(uname -s)" != Darwin || "$(uname -m)" != arm64 ]] || ! command -v container >/dev/null 2>&1; then
  echo "cabinet codegen analysis requires Apple container on Apple Silicon" >&2
  exit 1
fi

container build --arch arm64 --file "$ROOT/Dockerfile.cross-armv7" --tag "$APPLE_IMAGE" "$ROOT" >/dev/null
container run --arch arm64 --rm \
  --cpus "$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.logicalcpu)" \
  --memory 8g \
  --volume "$ROOT:/project" \
  --workdir /project \
  "$APPLE_IMAGE" \
  bash -lc "arm-linux-gnueabihf-nm -S --size-sort -C '$REL_BIN' >'$REL_OUT/symbols.tsv'; arm-linux-gnueabihf-objdump -d -C '$REL_BIN' >'$REL_OUT/disassembly.txt'"

grep -Ei 'cabinet|arcade.*formation|particle.*project' "$OUT_DIR/symbols.tsv" >"$OUT_DIR/cabinet-symbols.tsv" || true
grep -Eo '\b(vld[1-4]|vst[1-4]|vmla|vmls|vmul|vadd|vsub|vrecpe|vrecps|vcvt)(\.[a-z0-9]+)?\b' \
  "$OUT_DIR/disassembly.txt" | sort | uniq -c >"$OUT_DIR/vector-instructions.tsv" || true
grep -E '__aeabi_fdiv|panic|bounds_check' "$OUT_DIR/disassembly.txt" >"$OUT_DIR/suspicious-calls.txt" || true

printf 'revision\t%s\n' "$REVISION" >"$OUT_DIR/summary.tsv"
printf 'binary\t%s\n' "$BIN" >>"$OUT_DIR/summary.tsv"
printf 'cabinet_symbols\t%s\n' "$(wc -l <"$OUT_DIR/cabinet-symbols.tsv" | tr -d ' ')" >>"$OUT_DIR/summary.tsv"
printf 'suspicious_calls\t%s\n' "$(wc -l <"$OUT_DIR/suspicious-calls.txt" | tr -d ' ')" >>"$OUT_DIR/summary.tsv"
printf '%s\n' "$OUT_DIR"
