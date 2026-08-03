#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(cd "$APP_DIR/../.." && pwd)"
BIN="${1:-$APP_DIR/target/armv7-unknown-linux-gnueabihf/release-device-profile/mister-magik-framebuffer-scene-lab}"
RELEASE_BIN="${BIN/release-device-profile/release-device}"
REVISION="$(git -C "$ROOT" rev-parse HEAD)"
OUT_DIR="$ROOT/build/cabinet-codegen/$REVISION"
APPLE_IMAGE="${MISTER_APPLE_CONTAINER_IMAGE:-mister-magik-cross-armv7:ubuntu20-arm64}"

if [[ ! -f "$BIN" || ! -f "$RELEASE_BIN" || "$BIN" != "$ROOT/"* || "$RELEASE_BIN" != "$ROOT/"* ]]; then
  echo "cabinet analysis requires a linked scene-lab binary under $ROOT: $BIN" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
REL_BIN="/project/${BIN#"$ROOT/"}"
REL_RELEASE_BIN="/project/${RELEASE_BIN#"$ROOT/"}"
REL_OUT="/project/${OUT_DIR#"$ROOT/"}"

if [[ "$(uname -s)" != Darwin || "$(uname -m)" != arm64 ]] || ! command -v container >/dev/null 2>&1; then
  echo "cabinet codegen analysis requires Apple container on Apple Silicon" >&2
  exit 1
fi

container run --arch arm64 --rm \
  --cpus "$(getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.logicalcpu)" \
  --memory 8g \
  --volume "$ROOT:/project" \
  --workdir /project \
  "$APPLE_IMAGE" \
  bash -lc "arm-linux-gnueabihf-nm -S --size-sort -C '$REL_BIN' >'$REL_OUT/symbols.tsv'; arm-linux-gnueabihf-objdump -d -C '$REL_BIN' >'$REL_OUT/disassembly.txt'; arm-linux-gnueabihf-objdump -d -C '$REL_RELEASE_BIN' >'$REL_OUT/release-disassembly.txt'"

grep -Ei 'cabinet|arcade.*formation|particle.*project' "$OUT_DIR/symbols.tsv" >"$OUT_DIR/cabinet-symbols.tsv" || true
awk '/<mister_magik_cabinet_neon_project_stable>:/,/^$/' \
  "$OUT_DIR/disassembly.txt" >"$OUT_DIR/hot-symbol-profile.txt"
awk '/<mister_magik_cabinet_neon_project_stable>:/,/^$/' \
  "$OUT_DIR/release-disassembly.txt" >"$OUT_DIR/hot-symbol-release.txt"
if [[ ! -s "$OUT_DIR/hot-symbol-profile.txt" || ! -s "$OUT_DIR/hot-symbol-release.txt" ]]; then
  echo "linked cabinet NEON hot symbol is missing" >&2
  exit 1
fi
grep -Eo '\b(vld[1-4]|vst[1-4]|vmla|vmls|vmul|vadd|vsub|vrecpe|vrecps|vcvt)(\.[a-z0-9]+)?\b' \
  "$OUT_DIR/hot-symbol-profile.txt" | sort | uniq -c >"$OUT_DIR/vector-instructions.tsv" || true
grep -E '\b(bl|blx)\b' "$OUT_DIR/hot-symbol-profile.txt" >"$OUT_DIR/hot-calls.txt" || true
grep -E '__aeabi_fdiv|alloc|panic|format|bounds_check' \
  "$OUT_DIR/hot-symbol-profile.txt" >"$OUT_DIR/suspicious-calls.txt" || true
for flavor in release profile; do
  sed -E \
    -e 's/^[[:space:]]*[0-9a-f]+:/ADDR:/' \
    -e 's/[0-9a-f]+ <[^>]+>/TARGET/g' \
    "$OUT_DIR/hot-symbol-$flavor.txt" >"$OUT_DIR/hot-symbol-$flavor.normalized.txt"
done
diff -u \
  "$OUT_DIR/hot-symbol-release.normalized.txt" \
  "$OUT_DIR/hot-symbol-profile.normalized.txt" \
  >"$OUT_DIR/release-profile.diff" || true

printf 'revision\t%s\n' "$REVISION" >"$OUT_DIR/summary.tsv"
printf 'binary\t%s\n' "$BIN" >>"$OUT_DIR/summary.tsv"
printf 'release_binary\t%s\n' "$RELEASE_BIN" >>"$OUT_DIR/summary.tsv"
printf 'cabinet_symbols\t%s\n' "$(wc -l <"$OUT_DIR/cabinet-symbols.tsv" | tr -d ' ')" >>"$OUT_DIR/summary.tsv"
printf 'hot_symbol_size_hex\t%s\n' "$(awk '$4 == \"mister_magik_cabinet_neon_project_stable\" {print $2}' "$OUT_DIR/symbols.tsv")" >>"$OUT_DIR/summary.tsv"
printf 'hot_calls\t%s\n' "$(wc -l <"$OUT_DIR/hot-calls.txt" | tr -d ' ')" >>"$OUT_DIR/summary.tsv"
printf 'suspicious_calls\t%s\n' "$(wc -l <"$OUT_DIR/suspicious-calls.txt" | tr -d ' ')" >>"$OUT_DIR/summary.tsv"
printf 'release_profile_diff_lines\t%s\n' "$(wc -l <"$OUT_DIR/release-profile.diff" | tr -d ' ')" >>"$OUT_DIR/summary.tsv"
printf '%s\n' "$OUT_DIR"
