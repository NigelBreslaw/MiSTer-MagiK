#!/usr/bin/env bash
# Run MiSTer screenshot media save/publish benchmarks and append tool output rows.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-screenshot-save.tsv"

LABEL=""
SYSTEM="neogeo"
ITERATIONS=1
SIZE_BYTES=""
REPLACE_LABEL=0
REMOTE_BIN="${MISTER_MAGIK_REMOTE_BIN:-/media/fat/mister-magik/mister-magik-fb}"

usage() {
  cat <<'EOF'
Usage: scripts/profile-screenshot-save.sh LABEL --system ID [--iterations N] [--size-bytes BYTES] [--replace-label]

Benchmarks screenshot pack save/publish paths inside a deployed bench-tools
MagiK binary on the MiSTer. Build with `magik-gui/build-arm.sh --bench-tools`
before deploying this benchmark binary. This excludes network download,
decompression, and checksum work so the progress-capable save path can be
measured directly.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --system) SYSTEM="${2:?}"; shift 2 ;;
    --iterations) ITERATIONS="${2:?}"; shift 2 ;;
    --modes) echo "--modes was removed; screenshot saving has one progress-capable path" >&2; exit 2 ;;
    --size-bytes) SIZE_BYTES="${2:?}"; shift 2 ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      if [[ -n "$LABEL" ]]; then
        echo "unexpected argument: $1" >&2
        usage >&2
        exit 2
      fi
      LABEL="$1"
      shift
      ;;
  esac
done

if [[ -z "$LABEL" ]]; then
  LABEL="screenshot-save-$(date -u +%Y%m%dT%H%M%SZ)"
fi
if [[ ! "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$ITERATIONS" =~ ^[0-9]+$ || "$ITERATIONS" -lt 1 ]]; then
  echo "iterations must be a positive integer" >&2
  exit 2
fi
if [[ -n "$SIZE_BYTES" && ( ! "$SIZE_BYTES" =~ ^[0-9]+$ || "$SIZE_BYTES" -lt 1 ) ]]; then
  echo "size-bytes must be a positive integer" >&2
  exit 2
fi
mkdir -p "$BENCH_DIR"
if [[ "$REPLACE_LABEL" -eq 1 && -f "$TSV" ]]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^screenshot_save_bench_tsv	${LABEL}	" "$TSV" | tail -n +2; } >"$tmp" || true
  mv "$tmp" "$TSV"
fi

shell_quote() {
  printf "'%s'" "${1//\'/\'\\\'\'}"
}

remote_cmd="$(shell_quote "$REMOTE_BIN") media-bench-save"
remote_cmd+=" --label $(shell_quote "$LABEL")"
remote_cmd+=" --system $(shell_quote "$SYSTEM")"
remote_cmd+=" --iterations $(shell_quote "$ITERATIONS")"
if [[ -n "$SIZE_BYTES" ]]; then
  remote_cmd+=" --size-bytes $(shell_quote "$SIZE_BYTES")"
fi

out="$("$MISTER" run "$remote_cmd")"
printf '%s\n' "$out"
if [[ ! -f "$TSV" ]]; then
  printf 'type\tlabel\tsystem\tmode\titeration\tbytes\tcopy_ms\tsync_ms\trename_ms\tparent_sync_ms\ttotal_ms\tprogress_events\tresult\n' >"$TSV"
fi
printf '%s\n' "$out" | grep '^screenshot_save_bench_tsv	' >>"$TSV" || true
echo "appended to $TSV"
