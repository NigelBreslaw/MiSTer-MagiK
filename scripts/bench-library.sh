#!/usr/bin/env bash
# Benchmark MiSTer library scan/index performance on the device.
#
#   scripts/bench-library.sh LIB-BASE-20260605 --device --replace-label
#
# Appends library_scan_bench_tsv rows to history/toolchain-bench/results-library.tsv.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$HERE/magik-gui"
BUILD_PROFILE=release
BUILD_FLAG=()
REMOTE="/media/fat/mister-magic/mister-magic-fb"
BENCH_SQLITE="/media/fat/mister-magic/library-scan-bench.sqlite3"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-library.tsv"
ITERATIONS=3
LABEL="LIB-BENCH"
DO_CLEAN=0
SKIP_BUILD=0
REPLACE_LABEL=0

usage() {
  sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
  echo ""
  echo "Options: --clean  --skip-build  --replace-label  --device  --iterations N  -h"
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage 0 ;;
    --clean) DO_CLEAN=1; shift ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    --device) BUILD_PROFILE=release-device; BUILD_FLAG=(--device); shift ;;
    --iterations) ITERATIONS="${2:?}"; shift 2 ;;
    -*) echo "Unknown option: $1" >&2; usage 1 ;;
    *) LABEL="$1"; shift ;;
  esac
done

BIN="$RUST_DIR/target/armv7-unknown-linux-gnueabihf/$BUILD_PROFILE/mister-magic-fb"
mkdir -p "$BENCH_DIR"

if [[ ! -f "$TSV" ]]; then
  echo "label	iteration	scenario	us	notes" > "$TSV"
elif [[ "$REPLACE_LABEL" -eq 1 ]]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^${LABEL}	" "$TSV" | tail -n +2; } > "$tmp" || true
  mv "$tmp" "$TSV"
fi

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "== build ($BUILD_PROFILE) =="
  if [[ "$DO_CLEAN" -eq 1 ]]; then
    (cd "$RUST_DIR" && ./build-arm.sh "${BUILD_FLAG[@]}" --clean)
  else
    (cd "$RUST_DIR" && ./build-arm.sh "${BUILD_FLAG[@]}")
  fi
fi

echo "== deploy =="
"$HERE/scripts/mister" run 'kill -9 $(pidof mister-magic-fb) 2>/dev/null || true; mkdir -p /media/fat/mister-magic'
"$HERE/scripts/mister" put "$BIN" "$REMOTE"

echo "== library-scan-bench on device =="
OUT=$("$HERE/scripts/mister" run "chmod +x $REMOTE; MISTER_LIBRARY_BENCH_LABEL=$LABEL MISTER_LIBRARY_BENCH_ITERATIONS=$ITERATIONS MISTER_LIBRARY_BENCH_SQLITE=$BENCH_SQLITE MISTER_LIBRARY_SQLITE=$BENCH_SQLITE MISTER_LIBRARY_OPTIONAL_CATALOGS=1 $REMOTE library-scan-bench" 2>&1) || true
echo "$OUT"

echo "$OUT" | awk -F '\t' '$1 == "library_scan_bench_tsv" { print $2 "\t" $3 "\t" $4 "\t" $5 "\t" $6 }' >> "$TSV"
echo "appended to $TSV"
