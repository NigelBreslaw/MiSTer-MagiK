#!/usr/bin/env bash
# Measure launcher responsiveness during a simulated slow/failing Main handoff.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
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
Usage: scripts/profile-launch-handoff.sh LABEL [--replace-label] [--iterations N] [--delay-ms N] [--mode slow-fail] [--deploy-device]

Runs the real launcher loading/recovery path with a benchmark-only simulated
Main/FIFO handoff. It never writes /dev/MiSTer_cmd and never loads a core.
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
if [[ "$MODE" != "slow-fail" ]]; then
  echo "--mode currently supports only slow-fail" >&2
  exit 2
fi

mkdir -p "$BENCH_DIR" "$HERE/build/launch-handoff"
if [[ ! -f "$TSV" ]]; then
  echo "label	iteration	launch_action_to_loading_us	max_frame_gap_us	loading_frames_before_result	failure_recovery_us	launch_prep_us	handoff_wait_us	result" >"$TSV"
elif [[ "$REPLACE_LABEL" -eq 1 ]]; then
  tmp="$(mktemp)"
  { head -1 "$TSV"; grep -v "^${LABEL}	" "$TSV" | tail -n +2; } >"$tmp" || true
  mv "$tmp" "$TSV"
fi

if [[ "$DEPLOY" -eq 1 ]]; then
  "$HERE/scripts/deploy-rust.sh" --device --ui-scope launcher
fi

remote_trace="/tmp/${LABEL}-launch-handoff.tsv"
local_trace="$HERE/build/launch-handoff/${LABEL}.tsv"
local_log="$HERE/build/launch-handoff/${LABEL}.log"
env_file="$(mktemp)"

cleanup() {
  rm -f "$env_file"
  "$MISTER" run "rm -f '$REMOTE_ENV'; if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}
trap cleanup EXIT

{
  printf 'export MISTER_CATALOG_REFRESH=off\n'
  printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
  printf 'export MISTER_LAUNCHER_LOCK_SCREEN=arcade\n'
  printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=launch-handoff\n'
  printf 'export MISTER_LAUNCH_HANDOFF_LABEL=%q\n' "$LABEL"
  printf 'export MISTER_LAUNCH_HANDOFF_TRACE=%q\n' "$remote_trace"
  printf 'export MISTER_LAUNCH_HANDOFF_ITERATIONS=%q\n' "$ITERATIONS"
  printf 'export MISTER_LAUNCH_HANDOFF_DELAY_MS=%q\n' "$DELAY_MS"
} >"$env_file"

echo "== launch handoff profile label=$LABEL mode=$MODE iterations=$ITERATIONS delay_ms=$DELAY_MS"
"$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
"$MISTER" run "rm -f '$REMOTE_LOG' '$remote_trace'; if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd" >/dev/null

sleep_secs=$((8 + ITERATIONS * (DELAY_MS / 1000 + 2)))
sleep "$sleep_secs"

if ! "$MISTER" get "$remote_trace" "$local_trace" >/dev/null; then
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null || true
  echo "launch handoff benchmark failed; see $local_log" >&2
  exit 1
fi
"$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null || true
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
    print label, $3, action[2], gap[2], frames[2], recovery[2], prep[2], wait[2], result[2]
  }
' "$local_trace" >>"$TSV"

echo
echo $'launch_handoff\tlabel\titeration\tlaunch_action_to_loading_us\tmax_frame_gap_us\tloading_frames_before_result\tfailure_recovery_us\tlaunch_prep_us\thandoff_wait_us\tresult'
awk -F '\t' '
  $1 == "launch_handoff_sample" {
    split($4, action, "=")
    split($5, gap, "=")
    split($6, frames, "=")
    split($7, recovery, "=")
    split($8, prep, "=")
    split($9, wait, "=")
    split($10, result, "=")
    print "launch_handoff\t" $2 "\t" $3 "\t" action[2] "\t" gap[2] "\t" frames[2] "\t" recovery[2] "\t" prep[2] "\t" wait[2] "\t" result[2]
  }
' "$local_trace"

echo "appended to $TSV"
