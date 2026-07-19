#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Measure launcher launch-ref preparation without loading a core.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE="/media/fat/mister-magik-dev/mister-magik-fb"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-launch-prep.tsv"
LABEL=""
SCENARIO="warm"
ITERATIONS=5
REPLACE_LABEL=0

usage() {
  cat <<'EOF'
Usage: scripts/profile-launch-prep.sh LABEL [--replace-label] [--scenario warm|cold|priority-prewarm] [--iterations N]

Runs the launch-prep benchmark on the MiSTer. Requires a deployed bench-tools
MagiK binary built with `apps/mister/build-arm.sh --bench-tools`. The benchmark
calls the launch ref preparation path only; it does not write the MiSTer FIFO
or launch a core.
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
HEADER=$'label\tscenario\ttype\titeration\tref_index\tkind\tstatus\tprepare_us\tread_bytes\trchar\tsyscr\twrite_bytes\twchar\tsyscw\tdescriptor_written\tdescriptor_skipped\tdescriptor_bytes\tnotes'
if [[ ! -f "$TSV" ]]; then
  printf '%s\n' "$HEADER" >"$TSV"
elif [[ "$(head -1 "$TSV")" != "$HEADER" ]]; then
  tmp="$(mktemp)"
  { printf '%s\n' "$HEADER"; tail -n +2 "$TSV"; } >"$tmp"
  mv "$tmp" "$TSV"
fi
if [[ "$REPLACE_LABEL" -eq 1 ]]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^${LABEL}	" "$TSV" | tail -n +2; } >"$tmp" || true
  mv "$tmp" "$TSV"
fi

REMOTE_ENV=""
for var in \
  MISTER_LAUNCH_PREP_VIRTUAL_SYSTEMS \
  MISTER_LAUNCH_PREP_VIRTUAL_LIMIT \
  MISTER_LAUNCH_PREP_AMIGAVISION_LIMIT; do
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
    split($9, rb, "=")
    split($10, rc, "=")
    split($11, sr, "=")
    split($12, wb, "=")
    split($13, wc, "=")
    split($14, sw, "=")
    split($15, dw, "=")
    split($16, ds, "=")
    split($17, db, "=")
    print label, $3, "sample", $4, $5, $6, $7, $8, rb[2], rc[2], sr[2], wb[2], wc[2], sw[2], dw[2], ds[2], db[2], $18 " " $19
  }
  $1 == "launch_prep_bench_prewarm_tsv" {
    split($6, total, "=")
    split($7, written, "=")
    split($8, unchanged, "=")
    split($9, errors, "=")
    split($10, prewarm, "=")
    split($11, rb, "=")
    split($12, rc, "=")
    split($13, sr, "=")
    split($14, wb, "=")
    split($15, wc, "=")
    split($16, sw, "=")
    print label, $3, "prewarm", $4, "", "", $5, prewarm[2], rb[2], rc[2], sr[2], wb[2], wc[2], sw[2], 0, 0, 0, "total=" total[2] " written=" written[2] " unchanged=" unchanged[2] " errors=" errors[2]
  }
  $1 == "launch_prep_bench_summary" {
    split($4, count, "=")
    split($5, errors, "=")
    split($6, p50, "=")
    split($7, p95, "=")
    split($8, rb, "=")
    split($9, rc, "=")
    split($10, sr, "=")
    split($11, wb, "=")
    split($12, wc, "=")
    split($13, sw, "=")
    split($14, dw, "=")
    split($15, ds, "=")
    split($16, db, "=")
    print label, $3, "summary", "", "", "", "errors=" errors[2] " count=" count[2], "", rb[2], rc[2], sr[2], wb[2], wc[2], sw[2], dw[2], ds[2], db[2], "p50_us=" p50[2] " p95_us=" p95[2]
  }
' >>"$TSV"

echo "appended to $TSV"
