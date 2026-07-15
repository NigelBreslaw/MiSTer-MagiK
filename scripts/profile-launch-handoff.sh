#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Measure launcher responsiveness during a simulated slow/failing Main handoff.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_ENV="/media/fat/mister-magik-dev/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-launch-handoff.tsv"

LABEL=""
ITERATIONS=1
DELAY_MS=750
MODE="slow-fail"
REPLACE_LABEL=0
DEPLOY=0

usage() {
  cat <<'EOF'
Usage: scripts/profile-launch-handoff.sh LABEL [--replace-label] [--iterations N] [--delay-ms N] [--mode slow-fail|success] [--deploy-device]

Runs the real launcher loading/recovery path with a benchmark-only simulated
Main/FIFO handoff. It never writes /dev/MiSTer_cmd and never loads a core.
Requires a deployed bench-tools MagiK binary; --deploy-device builds one.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --replace-label) REPLACE_LABEL=1; shift ;;
    --iterations) ITERATIONS="${2:?}"; shift 2 ;;
    --delay-ms) DELAY_MS="${2:?}"; shift 2 ;;
    --mode) MODE="${2:?}"; shift 2 ;;
    --deploy-device) DEPLOY=1; shift ;;
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
  LABEL="launch-handoff-$(date -u +%Y%m%dT%H%M%SZ)"
fi
if [[ ! "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$ITERATIONS" =~ ^[0-9]+$ || "$ITERATIONS" -lt 1 ]]; then
  echo "--iterations must be a positive integer" >&2
  exit 2
fi
if [[ ! "$DELAY_MS" =~ ^[0-9]+$ ]]; then
  echo "--delay-ms must be a non-negative integer" >&2
  exit 2
fi
if [[ "$MODE" != "slow-fail" && "$MODE" != "success" ]]; then
  echo "--mode supports slow-fail or success" >&2
  exit 2
fi

mkdir -p "$BENCH_DIR" "$HERE/build/launch-handoff"
HEADER=$'label\titeration\tlaunch_action_to_loading_us\tmax_frame_gap_us\tloading_frames_before_result\tfailure_recovery_us\tlaunch_prep_us\thandoff_wait_us\tresult\thandoff_complete_us\tfirst_ack_us\trecovery'
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

if [[ "$DEPLOY" -eq 1 ]]; then
  "$HERE/scripts/deploy-rust.sh" --device --ui-scope launcher --bench-tools
fi

remote_trace="/tmp/${LABEL}-launch-handoff.tsv"
local_trace="$HERE/build/launch-handoff/${LABEL}.tsv"
local_log="$HERE/build/launch-handoff/${LABEL}.log"
env_file="$(mktemp)"

count_trace_samples() {
  local path="$1"
  awk -F '\t' -v label="$LABEL" '
    $1 == "launch_handoff_sample" && $2 == label { count++ }
    END { print count + 0 }
  ' "$path" 2>/dev/null || echo 0
}

cleanup() {
  rm -f "$env_file"
  "$MISTER" run "rm -f '$REMOTE_ENV'; if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}
trap cleanup EXIT

{
  printf 'export MISTER_CATALOG_REFRESH=default\n'
  printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
  printf 'export MISTER_LAUNCHER_LOCK_SCREEN=arcade\n'
  printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=launch-handoff\n'
  printf 'export MISTER_LAUNCH_HANDOFF_LABEL=%q\n' "$LABEL"
  printf 'export MISTER_LAUNCH_HANDOFF_TRACE=%q\n' "$remote_trace"
  printf 'export MISTER_LAUNCH_HANDOFF_ITERATIONS=%q\n' "$ITERATIONS"
  printf 'export MISTER_LAUNCH_HANDOFF_DELAY_MS=%q\n' "$DELAY_MS"
  printf 'export MISTER_LAUNCH_HANDOFF_MODE=%q\n' "$MODE"
} >"$env_file"

echo "== launch handoff profile label=$LABEL mode=$MODE iterations=$ITERATIONS delay_ms=$DELAY_MS"
"$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
"$MISTER" run "rm -f '$REMOTE_LOG' '$remote_trace'; if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd" >/dev/null

wait_timeout_secs=$((30 + ITERATIONS * (((DELAY_MS + 999) / 1000) + 4)))
deadline=$((SECONDS + wait_timeout_secs))
sample_count=0
while (( SECONDS < deadline )); do
  if "$MISTER" get "$remote_trace" "$local_trace" >/dev/null 2>&1; then
    sample_count="$(count_trace_samples "$local_trace")"
    if (( sample_count >= ITERATIONS )); then
      break
    fi
  fi
  sleep 1
done

if ! "$MISTER" get "$remote_trace" "$local_trace" >/dev/null; then
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null || true
  echo "launch handoff benchmark failed; see $local_log" >&2
  exit 1
fi
"$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null || true
sample_count="$(count_trace_samples "$local_trace")"
if (( sample_count < ITERATIONS )); then
  echo "launch handoff benchmark emitted $sample_count of $ITERATIONS requested samples within ${wait_timeout_secs}s; see $local_log" >&2
  exit 1
fi
echo "wrote $local_trace"
echo "wrote $local_log"

awk -F '\t' -v label="$LABEL" '
  BEGIN { OFS = "\t" }
  $1 == "launch_handoff_sample" {
    split($4, action, "=")
    split($5, gap, "=")
    split($6, frames, "=")
    split($7, recovery, "=")
    split($8, prep, "=")
    split($9, wait, "=")
    split($10, result, "=")
    split($11, complete, "=")
    split($12, ack, "=")
    split($13, recovered, "=")
    print label, $3, action[2], gap[2], frames[2], recovery[2], prep[2], wait[2], result[2], complete[2], ack[2], recovered[2]
  }
' "$local_trace" >>"$TSV"

echo
echo $'launch_handoff\tlabel\titeration\tlaunch_action_to_loading_us\tmax_frame_gap_us\tloading_frames_before_result\tfailure_recovery_us\tlaunch_prep_us\thandoff_wait_us\tresult\thandoff_complete_us\tfirst_ack_us\trecovery'
awk -F '\t' '
  $1 == "launch_handoff_sample" {
    split($4, action, "=")
    split($5, gap, "=")
    split($6, frames, "=")
    split($7, recovery, "=")
    split($8, prep, "=")
    split($9, wait, "=")
    split($10, result, "=")
    split($11, complete, "=")
    split($12, ack, "=")
    split($13, recovered, "=")
    print "launch_handoff\t" $2 "\t" $3 "\t" action[2] "\t" gap[2] "\t" frames[2] "\t" recovery[2] "\t" prep[2] "\t" wait[2] "\t" result[2] "\t" complete[2] "\t" ack[2] "\t" recovered[2]
  }
' "$local_trace"

echo "appended to $TSV"
