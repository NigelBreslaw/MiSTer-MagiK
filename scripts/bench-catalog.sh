#!/usr/bin/env bash
# Benchmark arcade catalog pipeline on the MiSTer (walk, parse, merge, sort, PNG decode).
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-catalog.sh A0
#
# Appends one row to history/toolchain-bench/results-catalog.tsv.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$HERE/magik-gui"
BUILD_PROFILE=release
BUILD_FLAG=()
REMOTE="/media/fat/mister-magik/mister-magik-fb"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-catalog.tsv"
SSH="$HERE/scripts/mister_ssh.py"
SAMPLE_IMAGES=10

export MISTER_IP="${MISTER_IP:-192.168.1.117}"
export MISTER_PASS="${MISTER_PASS:-1}"

LABEL="A0"
DO_CLEAN=0
SKIP_BUILD=0
SKIP_DEVICE=0
REPLACE_LABEL=0

usage() {
  sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'
  echo ""
  echo "Options: --clean  --skip-build  --skip-device  --replace-label"
  echo "         --device  --sample-images N  -h"
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage 0 ;;
    --clean) DO_CLEAN=1; shift ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --skip-device) SKIP_DEVICE=1; shift ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    --device) BUILD_PROFILE=release-device; BUILD_FLAG=(--device); shift ;;
    --sample-images) SAMPLE_IMAGES="${2:?}"; shift 2 ;;
    -*) echo "Unknown option: $1" >&2; usage 1 ;;
    *) LABEL="$1"; shift ;;
  esac
done

BIN="$RUST_DIR/target/armv7-unknown-linux-gnueabihf/$BUILD_PROFILE/mister-magik-fb"
mkdir -p "$BENCH_DIR"

TSV_HEADER="label	date	rustc	compile_sec	bytes	walk_ms	walk_count	parse_ms	parse_count	merge_ms	merge_count	resolve_ms	resolve_count	sort_ms	sort_count	decode_ms	decode_count	total_ms	games	notes"

if [[ ! -f "$TSV" ]]; then
  echo "$TSV_HEADER" > "$TSV"
elif [[ "$REPLACE_LABEL" -eq 1 ]]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^${LABEL}	" "$TSV" | tail -n +2; } > "$tmp" || true
  mv "$tmp" "$TSV"
fi

DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
RUSTC="$(rustc --version 2>/dev/null | awk '{print $2}' || echo unknown)"

COMPILE_SEC=""
BYTES=""

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "== build ($BUILD_PROFILE) =="
  t0=$(date +%s)
  if [[ "$DO_CLEAN" -eq 1 ]]; then
    (cd "$RUST_DIR" && ./build-arm.sh "${BUILD_FLAG[@]}" --clean)
  else
    (cd "$RUST_DIR" && ./build-arm.sh "${BUILD_FLAG[@]}")
  fi
  t1=$(date +%s)
  COMPILE_SEC=$((t1 - t0))
  BYTES=$(stat -f%z "$BIN" 2>/dev/null || stat -c%s "$BIN")
  echo "compile ${COMPILE_SEC}s  bytes=$BYTES"
fi

if [[ "$SKIP_DEVICE" -eq 1 ]]; then
  echo "skip device (--skip-device)"
  exit 0
fi

echo "== deploy =="
MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" \
  uv run python "$SSH" put "$BIN" "$REMOTE"

echo "== catalog-bench on device =="
OUT=$(MISTER_IP="$MISTER_IP" MISTER_PASS="$MISTER_PASS" \
  uv run python "$SSH" run \
  "kill -9 \$(pidof mister-magik-fb) 2>/dev/null; $REMOTE catalog-bench --sample-images $SAMPLE_IMAGES" \
  2>&1) || true
echo "$OUT"

parse_phase() {
  local name="$1"
  echo "$OUT" | awk -v p="$name" '$1 == p { print $2, $3; exit }'
}

walk=$(parse_phase walk_mra); walk_ms=${walk%% *}; walk_count=${walk##* }
parse=$(parse_phase parse_gamelist); parse_ms=${parse%% *}; parse_count=${parse##* }
merge=$(parse_phase merge_entries); merge_ms=${merge%% *}; merge_count=${merge##* }
resolve=$(parse_phase resolve_images); resolve_ms=${resolve%% *}; resolve_count=${resolve##* }
sort=$(parse_phase sort_catalog); sort_ms=${sort%% *}; sort_count=${sort##* }
decode=$(parse_phase decode_sample_pngs); decode_ms=${decode%% *}; decode_count=${decode##* }
total_ms=$(echo "$OUT" | awk '/^total / { print $2; exit }')
games=$(echo "$OUT" | awk '/^games=/ { sub(/^games=/, ""); print; exit }')

if [[ -z "$BYTES" && -f "$BIN" ]]; then
  BYTES=$(stat -f%z "$BIN" 2>/dev/null || stat -c%s "$BIN")
fi

echo "$LABEL	$DATE	$RUSTC	${COMPILE_SEC:-}	${BYTES:-}	${walk_ms:-}	${walk_count:-}	${parse_ms:-}	${parse_count:-}	${merge_ms:-}	${merge_count:-}	${resolve_ms:-}	${resolve_count:-}	${sort_ms:-}	${sort_count:-}	${decode_ms:-}	${decode_count:-}	${total_ms:-}	${games:-}	sample=$SAMPLE_IMAGES" >> "$TSV"
echo "appended to $TSV"
