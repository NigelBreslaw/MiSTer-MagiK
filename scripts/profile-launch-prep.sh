#!/usr/bin/env bash
# Measure launcher launch-ref preparation without loading a core.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE="/media/fat/mister-magik/mister-magik-fb"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-launch-prep.tsv"
LABEL=""
SCENARIO="warm"
ITERATIONS=5
REPLACE_LABEL=0

usage() {
  cat <<'EOF'
Usage: scripts/profile-launch-prep.sh LABEL [--replace-label] [--scenario warm|cold|priority-prewarm] [--iterations N]

Runs the launch-prep benchmark on the MiSTer. The benchmark calls the launch
ref preparation path only; it does not write the MiSTer FIFO or launch a core.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --replace-label) REPLACE_LABEL=1; shift ;;
    --scenario) SCENARIO="${2:?}"; shift 2 ;;
    --iterations) ITERATIONS="${2:?}"; shift 2 ;;
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
  LABEL="launch-prep-$(date -u +%Y%m%dT%H%M%SZ)"
fi
if [[ ! "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ "$SCENARIO" != "warm" && "$SCENARIO" != "cold" && "$SCENARIO" != "priority-prewarm" ]]; then
  echo "--scenario must be warm, cold, or priority-prewarm" >&2
  exit 2
fi
if [[ ! "$ITERATIONS" =~ ^[0-9]+$ || "$ITERATIONS" -lt 1 ]]; then
  echo "--iterations must be a positive integer" >&2
  exit 2
fi

mkdir -p "$BENCH_DIR"
if [[ ! -f "$TSV" ]]; then
  echo "label	scenario	type	iteration	ref_index	kind	status	prepare_us	write_bytes	wchar	notes" >"$TSV"
elif [[ "$REPLACE_LABEL" -eq 1 ]]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^${LABEL}	" "$TSV" | tail -n +2; } >"$tmp" || true
  mv "$tmp" "$TSV"
fi

REMOTE_ENV=""
for var in \
  MISTER_LAUNCH_PREP_VIRTUAL_SYSTEMS \
  MISTER_LAUNCH_PREP_VIRTUAL_LIMIT \
  MISTER_LAUNCH_PREP_AMIGAVISION_LIMIT \
  MISTER_LAUNCH_CACHE_PRIORITY_SYSTEMS \
  MISTER_LAUNCH_CACHE_PRIORITY_PER_SYSTEM; do
  value="${!var-}"
  if [[ -n "$value" ]]; then
    if [[ ! "$value" =~ ^[A-Za-z0-9_.:/\|,-]+$ ]]; then
      echo "$var contains unsupported characters for remote benchmark passthrough" >&2
      exit 2
    fi
    REMOTE_ENV+=" $var='$value'"
  fi
done

echo "== launch prep profile label=$LABEL scenario=$SCENARIO iterations=$ITERATIONS"
OUT=$("$MISTER" run "chmod +x '$REMOTE';$REMOTE_ENV MISTER_LAUNCH_PREP_LABEL='$LABEL' MISTER_LAUNCH_PREP_ITERATIONS='$ITERATIONS' '$REMOTE' launch-prep-bench '$LABEL' '$SCENARIO' '$ITERATIONS'" 2>&1) || true
echo "$OUT"

echo "$OUT" | awk -F '\t' -v label="$LABEL" '
  BEGIN { OFS = "\t" }
  $1 == "launch_prep_bench_tsv" {
    split($9, wb, "=")
    split($10, wc, "=")
    print label, $3, "sample", $4, $5, $6, $7, $8, wb[2], wc[2], $11 " " $12
  }
  $1 == "launch_prep_bench_prewarm_tsv" {
    split($6, total, "=")
    split($7, written, "=")
    split($8, unchanged, "=")
    split($9, errors, "=")
    split($10, prewarm, "=")
    split($11, wb, "=")
    split($12, wc, "=")
    print label, $3, "prewarm", $4, "", "", $5, prewarm[2], wb[2], wc[2], "total=" total[2] " written=" written[2] " unchanged=" unchanged[2] " errors=" errors[2]
  }
  $1 == "launch_prep_bench_summary" {
    split($4, count, "=")
    split($5, errors, "=")
    split($6, p50, "=")
    split($7, p95, "=")
    split($8, wb, "=")
    split($9, wc, "=")
    print label, $3, "summary", "", "", "", "errors=" errors[2] " count=" count[2], "", p50[2], p95[2], "write_bytes=" wb[2] " wchar=" wc[2]
  }
' >>"$TSV"

echo "appended to $TSV"
