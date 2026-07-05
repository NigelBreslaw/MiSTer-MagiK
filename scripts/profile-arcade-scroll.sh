#!/usr/bin/env bash
# Capture a real launcher Arcade velocity-scroll trace through MiSTer_MagiK.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/arcade-scroll-profiles"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"
source "$HERE/scripts/thread-sampler-lib.sh"

usage() {
  cat <<'EOF'
Usage: scripts/profile-arcade-scroll.sh [LABEL] [--secs N] [--scenario held-scroll|turbo-hold|velocity-scroll] [--skip-build|--deploy-device] [--thread-sample] [--selection-invert on|off] [--ui-fb-size auto|960x540|1280x720] [--present-delay-us N] [--stream-consumer none|desktop-bench|null-drain]

Legacy positional form is still accepted:
  scripts/profile-arcade-scroll.sh [SECS] [LABEL]

Runs the Main-supervised launcher on the real Arcade screen with
MISTER_LAUNCHER_BENCH_SCENARIO and MISTER_PREVIEW_SCROLL_TRACE,
pulls the raw TSV/log, then prints frame timing summaries.
Requires a deployed bench-tools MagiK binary; --deploy-device builds one.
--thread-sample records /proc per-thread CPU/core/scheduler samples once per
second while the timed scenario runs.
--selection-invert on|off toggles selected-row inversion for A/B cost runs.
--stream-consumer starts a desktop framebuffer stream consumer during the
timed window. desktop-bench decodes/RGBA-converts frames; null-drain reads the
binary stream without desktop image conversion.

Do not use row-step `list-scroll` for arcade performance benchmarking. It does
not reproduce real velocity scrolling.

Default: --skip-build, useful when the desired binary is already deployed.
EOF
}

secs="30"
label="arcade-scroll-$(date -u +%Y%m%dT%H%M%SZ)"
scenario="turbo-hold"
deploy="skip"
selection_invert=""
ui_fb_size="${MISTER_UI_FB_SIZE:-auto}"
present_delay_us="${MISTER_FB_PRESENT_DELAY_US:-0}"
stream_consumer="${MISTER_FRAMEBUFFER_STREAM_CONSUMER:-none}"
positionals=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) deploy="skip"; shift ;;
    --deploy-device) deploy="device"; shift ;;
    --thread-sample) thread_sample_enabled="1"; shift ;;
    --secs)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--secs needs a value" >&2; usage >&2; exit 2; fi
      secs="$2"
      shift 2
      ;;
    --scenario)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--scenario needs a value" >&2; usage >&2; exit 2; fi
      scenario="$2"
      shift 2
      ;;
    --selection-invert)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--selection-invert needs on or off" >&2; usage >&2; exit 2; fi
      selection_invert="$2"
      shift 2
      ;;
    --ui-fb-size)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--ui-fb-size needs auto, 960x540, or 1280x720" >&2; usage >&2; exit 2; fi
      ui_fb_size="$2"
      shift 2
      ;;
    --present-delay-us)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--present-delay-us needs a non-negative integer" >&2; usage >&2; exit 2; fi
      present_delay_us="$2"
      shift 2
      ;;
    --stream-consumer)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--stream-consumer needs none, desktop-bench, or null-drain" >&2; usage >&2; exit 2; fi
      stream_consumer="$2"
      shift 2
      ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) positionals+=("$1"); shift ;;
  esac
done

if [[ "${#positionals[@]}" -gt 2 ]]; then
  echo "unexpected argument: ${positionals[2]}" >&2
  usage >&2
  exit 2
fi
if [[ "${#positionals[@]}" -ge 1 ]]; then
  if [[ "${positionals[0]}" =~ ^[0-9]+$ ]]; then
    secs="${positionals[0]}"
    if [[ "${#positionals[@]}" -ge 2 ]]; then label="${positionals[1]}"; fi
  else
    label="${positionals[0]}"
    if [[ "${#positionals[@]}" -ge 2 ]]; then
      echo "unexpected argument after LABEL: ${positionals[1]}" >&2
      usage >&2
      exit 2
    fi
  fi
fi

if [[ ! "$secs" =~ ^[0-9]+$ ]]; then echo "secs must be an integer number of seconds" >&2; exit 2; fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then echo "label must contain only letters, numbers, _, ., or -" >&2; exit 2; fi
case "$scenario" in
  velocity-scroll|held-scroll|turbo-hold) ;;
  list-scroll|smooth-scroll|selected-first|stress-scroll|cache-warm|preview|preview-changes|screenshot-stress|preview-stress)
    echo "row-step/jump scenario '$scenario' is not valid for arcade benchmarking; use velocity-scroll, held-scroll, or turbo-hold" >&2
    exit 2
    ;;
  *) echo "unknown scenario: $scenario" >&2; usage >&2; exit 2 ;;
esac
case "$selection_invert" in
  ""|on|off) ;;
  *) echo "--selection-invert must be on or off" >&2; usage >&2; exit 2 ;;
esac
case "$ui_fb_size" in
  auto|960x540|1280x720) ;;
  *) echo "--ui-fb-size must be auto, 960x540, or 1280x720" >&2; exit 2 ;;
esac
if [[ ! "$present_delay_us" =~ ^[0-9]+$ ]]; then
  echo "--present-delay-us must be a non-negative integer" >&2
  exit 2
fi
case "$stream_consumer" in
  none|desktop-bench|null-drain) ;;
  *) echo "--stream-consumer must be none, desktop-bench, or null-drain" >&2; exit 2 ;;
esac
remote_scenario="$scenario"
if [[ "$remote_scenario" == "velocity-scroll" ]]; then remote_scenario="held-scroll"; fi

mkdir -p "$OUT_DIR"
remote_tsv="/tmp/${label}-arcade-scroll.tsv"
remote_log="$REMOTE_LOG"
local_tsv="$OUT_DIR/${label}-arcade-scroll.tsv"
local_log="$OUT_DIR/${label}-arcade-scroll.log"
local_status_json="$OUT_DIR/${label}-arcade-scroll.status.json"
local_stream_tsv="$OUT_DIR/${label}-framebuffer-stream.tsv"
local_stream_log="$OUT_DIR/${label}-framebuffer-stream.log"
env_file="$(mktemp "${TMPDIR:-/tmp}/mister-magik-arcade-scroll-env.XXXXXX")"
stream_pid=""
stream_frames=$((secs * 20))
if [[ "$stream_frames" -lt 1 ]]; then stream_frames=1; fi
present_width="960"
if [[ "$ui_fb_size" == "1280x720" ]]; then
  present_width="1280"
fi

check_composition_recovery_gate() {
  local status_json="$1"
  python3 - "$status_json" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
slint = data.get("runtime", {}).get("slint_status", {})
count = int(slint.get("composition_recovery_count") or 0)
state = slint.get("composition_state") or ""
kind = slint.get("last_composition_invariant_kind") or ""
detail = slint.get("last_composition_invariant_detail") or ""
print(
    f"composition_gate_tsv\tstate={state}\trecovery_count={count}\tlast_kind={kind}\tlast_detail={detail}\tvalid={1 if count == 0 else 0}"
)
raise SystemExit(0 if count == 0 else 11)
PY
}

start_stream_consumer() {
  case "$stream_consumer" in
    none) return 0 ;;
    desktop-bench) stream_arg="--framebuffer-stream-bench" ;;
    null-drain) stream_arg="--framebuffer-stream-drain-bench" ;;
  esac
  echo "==> Start framebuffer stream consumer mode=$stream_consumer frames=$stream_frames"
  (
    cd "$HERE"
    MISTER_IP="${MISTER_IP:-192.168.1.117}" cargo run --manifest-path desktop/Cargo.toml --locked -- "$stream_arg" "$stream_frames"
  ) >"$local_stream_tsv" 2>"$local_stream_log" &
  stream_pid="$!"
}

finish_stream_consumer() {
  if [[ -z "$stream_pid" ]]; then
    return 0
  fi
  if kill -0 "$stream_pid" >/dev/null 2>&1; then
    kill "$stream_pid" >/dev/null 2>&1 || true
    wait "$stream_pid" >/dev/null 2>&1 || true
    printf 'framebuffer_stream_bench_tsv\tmode=%s\tframes=%s\tcompleted=0\tinvalid_reason=consumer_timeout\n' \
      "$stream_consumer" "$stream_frames" | tee -a "$local_stream_tsv"
    return 14
  fi
  wait "$stream_pid"
}

cleanup() {
  rm -f "$env_file"
  if [[ -n "$stream_pid" ]] && kill -0 "$stream_pid" >/dev/null 2>&1; then
    kill "$stream_pid" >/dev/null 2>&1 || true
    wait "$stream_pid" >/dev/null 2>&1 || true
  fi
  "$MISTER" run "rm -f '$REMOTE_ENV'; if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}
trap cleanup EXIT

case "$deploy" in
  device) "$HERE/scripts/deploy-rust.sh" --device --ui-scope launcher --bench-tools ;;
  skip) : ;;
esac

echo "==> Capture supervised launcher Arcade scenario=$scenario remote_scenario=$remote_scenario secs=$secs label=$label deploy=$deploy ui_fb_size=$ui_fb_size present_delay_us=$present_delay_us stream_consumer=$stream_consumer"
{
  printf 'export MISTER_UI_FB_SIZE=%q\n' "$ui_fb_size"
  printf 'export MISTER_FB_PRESENT_DELAY_US=%q\n' "$present_delay_us"
  printf 'export MISTER_CATALOG_REFRESH=default\n'
  printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
  printf 'export MISTER_LAUNCHER_START_SYSTEM=arcade\n'
  printf 'export MISTER_LAUNCHER_LOCK_SCREEN=arcade\n'
  printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=%q\n' "$remote_scenario"
  printf 'export MISTER_PREVIEW_TRACE=1\n'
  printf 'export MISTER_PREVIEW_SCROLL_TRACE_SECS=%q\n' "$secs"
  printf 'export MISTER_PREVIEW_SCROLL_TRACE=%q\n' "$remote_tsv"
  if [[ "$selection_invert" == "off" ]]; then
    printf 'export MISTER_ARCADE_SELECTION_INVERT=0\n'
  elif [[ "$selection_invert" == "on" ]]; then
    printf 'export MISTER_ARCADE_SELECTION_INVERT=1\n'
  fi
} >"$env_file"
"$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
"$MISTER" run "rm -f '$remote_tsv' '$remote_log'; if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd" >/dev/null
start_stream_consumer
thread_sample_start "$label" "arcade-scroll" "$OUT_DIR" $((secs + 10))
sleep $((secs + 7))
thread_sample_finish
stream_status=0
finish_stream_consumer || stream_status="$?"

if ! "$MISTER" get "$remote_tsv" "$local_tsv" >/dev/null; then
  "$MISTER" get "$remote_log" "$local_log" >/dev/null || true
  echo "arcade scroll profile failed; see $local_log" >&2
  exit 1
fi
"$MISTER" get "$remote_log" "$local_log" >/dev/null || true
"$MISTER" status --json >"$local_status_json" 2>/dev/null || true

echo "wrote $local_tsv"
echo "wrote $local_log"
echo "wrote $local_status_json"
if [[ "$stream_consumer" != "none" ]]; then
  echo "wrote $local_stream_tsv"
  echo "wrote $local_stream_log"
  if [[ -s "$local_stream_tsv" ]]; then
    sed -n '1,20p' "$local_stream_tsv"
  fi
  if [[ "$stream_status" != "0" ]]; then
    echo "framebuffer stream consumer failed; see $local_stream_log" >&2
    exit "$stream_status"
  fi
fi
if [[ -s "$local_status_json" ]] && ! check_composition_recovery_gate "$local_status_json"; then
  echo "arcade scroll composition recovery occurred; see $local_status_json" >&2
  exit 13
fi
echo
"$HERE/scripts/analyze-arcade-frame-trace.py" "$local_tsv"
echo
"$HERE/scripts/launcher-present-trace.py" summarize "$local_tsv" --case arcade-scroll --present-width "$present_width"
