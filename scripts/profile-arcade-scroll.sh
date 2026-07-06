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
Usage: scripts/profile-arcade-scroll.sh [LABEL] [--secs N] [--scenario held-scroll|turbo-hold|velocity-scroll] [--skip-build|--deploy-device] [--thread-sample] [--skip-boot-prelude] [--entry-open-gate-ms N] [--entry-gate-ms N] [--selection-invert on|off] [--ui-fb-size auto|960x540|1280x720] [--present-delay-us N] [--stream-consumer none|desktop-bench|null-drain] [--self-test]

Legacy positional form is still accepted:
  scripts/profile-arcade-scroll.sh [SECS] [LABEL]

Runs the Main-supervised launcher on the real Arcade screen with
MISTER_LAUNCHER_BENCH_SCENARIO and MISTER_PREVIEW_SCROLL_TRACE. By default it
first reboots to Home, presses A on the Arcade tile, requires instant list,
preview, and first navigation readiness, then runs the 30s scroll trace.
Requires a deployed bench-tools MagiK binary; --deploy-device builds one.
--skip-boot-prelude keeps the old direct-to-Arcade benchmark setup.
--self-test runs only the host parser checks for the boot prelude gate.
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
boot_prelude="${MISTER_ARCADE_SCROLL_BOOT_PRELUDE:-1}"
entry_open_gate_ms="${MISTER_ARCADE_ENTRY_OPEN_GATE_MS:-350}"
entry_gate_ms="${MISTER_ARCADE_ENTRY_GATE_MS:-50}"
home_selected_index="${MISTER_ARCADE_ENTRY_HOME_SELECTED_INDEX:-7}"
self_test="0"
positionals=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) deploy="skip"; shift ;;
    --deploy-device) deploy="device"; shift ;;
    --thread-sample) thread_sample_enabled="1"; shift ;;
    --skip-boot-prelude|--direct-start-arcade) boot_prelude="0"; shift ;;
    --entry-open-gate-ms)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--entry-open-gate-ms needs a value" >&2; usage >&2; exit 2; fi
      entry_open_gate_ms="$2"
      shift 2
      ;;
    --entry-gate-ms)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--entry-gate-ms needs a value" >&2; usage >&2; exit 2; fi
      entry_gate_ms="$2"
      shift 2
      ;;
    --self-test) self_test="1"; shift ;;
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
if [[ ! "$entry_open_gate_ms" =~ ^[0-9]+$ ]]; then echo "--entry-open-gate-ms must be an integer" >&2; exit 2; fi
if [[ ! "$entry_gate_ms" =~ ^[0-9]+$ ]]; then echo "--entry-gate-ms must be an integer" >&2; exit 2; fi
if [[ ! "$home_selected_index" =~ ^[0-9]+$ ]]; then echo "MISTER_ARCADE_ENTRY_HOME_SELECTED_INDEX must be an integer" >&2; exit 2; fi
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
remote_entry_tsv="/tmp/${label}-arcade-entry.tsv"
local_entry_tsv="$OUT_DIR/${label}-arcade-entry.tsv"
local_entry_log="$OUT_DIR/${label}-arcade-entry.log"
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

check_preview_exact_gate() {
  local name="$1" trace="$2"
  python3 - "$name" "$trace" <<'PY'
import collections
import csv
import sys

name, trace_path = sys.argv[1:3]
try:
    with open(trace_path, encoding="utf-8") as f:
        rows = list(csv.DictReader(f, delimiter="\t"))
except FileNotFoundError:
    print(f"preview_exact_gate_tsv\tlabel={name}\tvalid=0\tinvalid_reason=missing_trace\tdetail={trace_path}")
    sys.exit(9)

if not rows:
    print(f"preview_exact_gate_tsv\tlabel={name}\tvalid=0\tinvalid_reason=no_frames\tdetail={trace_path}")
    sys.exit(9)
if "cache_state" not in rows[0]:
    print(f"preview_exact_gate_tsv\tlabel={name}\tvalid=0\tinvalid_reason=missing_column\tdetail=cache_state")
    sys.exit(9)

counts = collections.Counter(row.get("cache_state", "") for row in rows)
non_exact = [
    row for row in rows
    if row.get("cache_state") != "exact"
]
detail = " ".join(
    f"{state or 'blank'}={count}"
    for state, count in sorted(counts.items())
)
if non_exact:
    samples = []
    for row in non_exact[:10]:
        samples.append(
            f"frame={row.get('frame', '?')}:selected={row.get('selected', '?')}:cache_state={row.get('cache_state', '') or 'blank'}"
        )
    print(
        f"preview_exact_gate_tsv\tlabel={name}\tvalid=0\tinvalid_reason=non_exact_preview"
        f"\tdetail=frames={len(rows)} exact={counts.get('exact', 0)} non_exact={len(non_exact)} {detail}"
        f"\tsamples={','.join(samples)}"
    )
    sys.exit(9)

print(
    f"preview_exact_gate_tsv\tlabel={name}\tvalid=1\tinvalid_reason=ok"
    f"\tdetail=frames={len(rows)} exact={counts.get('exact', 0)} non_exact=0 {detail}"
)
PY
}

gate_arcade_entry_trace() {
  local name="$1" trace="$2" log="$3" open_threshold="$4" interactive_threshold="$5"
  python3 - "$name" "$trace" "$log" "$open_threshold" "$interactive_threshold" <<'PY'
import csv
import re
import sys

name, trace_path, log_path, open_threshold_s, interactive_threshold_s = sys.argv[1:6]
open_threshold = int(open_threshold_s)
interactive_threshold = int(interactive_threshold_s)
required = [
    "arcade_enter_input",
    "arcade_enter_presented",
    "arcade_rows_ready",
    "arcade_preview_exact",
    "arcade_first_nav_input",
    "arcade_first_nav_presented",
]
rows = {}
try:
    with open(trace_path, encoding="utf-8") as f:
        reader = csv.DictReader(f, delimiter="\t")
        for row in reader:
            rows.setdefault(row.get("event", ""), row)
except FileNotFoundError:
    print(f"arcade_entry_gate_tsv\tlabel={name}\tvalid=0\tinvalid_reason=missing_trace\tdetail={trace_path}")
    sys.exit(9)

missing = [event for event in required if event not in rows]
if missing:
    print(f"arcade_entry_gate_tsv\tlabel={name}\tvalid=0\tinvalid_reason=missing_event\tdetail={','.join(missing)}")
    sys.exit(9)

failures = []
input_delay = int(rows["arcade_enter_input"]["since_input_enabled_ms"])
if input_delay > open_threshold:
    failures.append(f"arcade_enter_input_since_input_enabled_ms={input_delay}")
open_delta = int(rows["arcade_enter_presented"]["delta_ms"])
if open_delta > open_threshold:
    failures.append(f"arcade_enter_presented_delta_ms={open_delta}")
open_prepare = rows["arcade_enter_presented"]["prepare_us"]
if open_prepare != "-" and int(open_prepare) > open_threshold * 1000:
    failures.append(f"arcade_enter_presented_prepare_us={open_prepare}")
for event in ("arcade_rows_ready", "arcade_preview_exact", "arcade_first_nav_presented"):
    delta = int(rows[event]["delta_ms"])
    if delta > interactive_threshold:
        failures.append(f"{event}_delta_ms={delta}")
prepare = rows["arcade_first_nav_presented"]["prepare_us"]
if prepare != "-" and int(prepare) > interactive_threshold * 1000:
    failures.append(f"arcade_first_nav_presented_prepare_us={prepare}")

try:
    log_text = open(log_path, encoding="utf-8", errors="replace").read()
except FileNotFoundError:
    log_text = ""
if re.search(r"startup_timing\tcatalog_navigation_load\t", log_text):
    failures.append("forbidden_catalog_navigation_load")
if re.search(r"startup_timing\tcatalog_system_navigation_load\t", log_text):
    failures.append("forbidden_catalog_system_navigation_load")
if re.search(r"startup_timing\tarcade_search_index_prewarm\t[^\n]*built=1", log_text):
    failures.append("forbidden_arcade_search_index_prewarm")

summary = " ".join(
    f"{event}_delta_ms={rows[event]['delta_ms']}"
    for event in required
)
summary += f" arcade_enter_input_since_input_enabled_ms={input_delay}"
summary += f" open_gate_ms={open_threshold} interactive_gate_ms={interactive_threshold}"
if failures:
    print(f"arcade_entry_gate_tsv\tlabel={name}\tvalid=0\tinvalid_reason=gate_failed\tdetail={';'.join(failures)} {summary}")
    sys.exit(9)
print(f"arcade_entry_gate_tsv\tlabel={name}\tvalid=1\tinvalid_reason=ok\tdetail={summary}")
PY
}

run_arcade_entry_self_test() {
  local tmpdir
  tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/arcade-entry-gate-self.XXXXXX")"
  trap 'rm -rf "${tmpdir:-}"' EXIT
  cat >"$tmpdir/good.tsv" <<'EOF'
event	elapsed_ms	delta_ms	since_input_enabled_ms	accepted	system	selected	frame	prepare_us	preview_state	asset_key	detail
arcade_enter_input	120	-1	8	1	arcade	0	-	-		1941	source=launcher_input
arcade_enter_presented	138	18	26	1	arcade	0	4	12000	placeholder	1941	copied_rows=540
arcade_rows_ready	139	19	27	1	arcade	0	-	-		1941	games=895
arcade_preview_exact	145	25	33	1	arcade	0	-	-	exact	1941	source=preview_state
arcade_first_nav_input	150	30	38	1	arcade	1	-	-		1942	source=launcher_input
arcade_first_nav_presented	166	16	54	1	arcade	1	5	13000	exact	1942	copied_rows=540
EOF
  : >"$tmpdir/good.log"
  gate_arcade_entry_trace self-good "$tmpdir/good.tsv" "$tmpdir/good.log" 350 50 >/dev/null
  cp "$tmpdir/good.tsv" "$tmpdir/slow.tsv"
  sed -i.bak $'s/arcade_rows_ready\t139\t19/arcade_rows_ready\t190\t70/' "$tmpdir/slow.tsv"
  if gate_arcade_entry_trace self-slow "$tmpdir/slow.tsv" "$tmpdir/good.log" 350 50 >/dev/null 2>&1; then
    echo "self-test expected latency failure" >&2
    exit 1
  fi
  cp "$tmpdir/good.tsv" "$tmpdir/slow-open.tsv"
  sed -i.bak $'s/arcade_enter_presented\t138\t18/arcade_enter_presented\t520\t400/' "$tmpdir/slow-open.tsv"
  if gate_arcade_entry_trace self-slow-open "$tmpdir/slow-open.tsv" "$tmpdir/good.log" 350 50 >/dev/null 2>&1; then
    echo "self-test expected open latency failure" >&2
    exit 1
  fi
  grep -v '^arcade_preview_exact' "$tmpdir/good.tsv" >"$tmpdir/missing.tsv"
  if gate_arcade_entry_trace self-missing "$tmpdir/missing.tsv" "$tmpdir/good.log" 350 50 >/dev/null 2>&1; then
    echo "self-test expected missing screenshot failure" >&2
    exit 1
  fi
  cat >"$tmpdir/exact-scroll.tsv" <<'EOF'
frame	selected	cache_state
0	0	exact
1	1	exact
EOF
  check_preview_exact_gate self-exact "$tmpdir/exact-scroll.tsv" >/dev/null
  cat >"$tmpdir/stale-scroll.tsv" <<'EOF'
frame	selected	cache_state
0	0	exact
1	1	stale
EOF
  if check_preview_exact_gate self-stale "$tmpdir/stale-scroll.tsv" >/dev/null 2>&1; then
    echo "self-test expected stale preview failure" >&2
    exit 1
  fi
  printf 'startup_timing\tcatalog_navigation_load\t100ms\tstatus=ready\n' >"$tmpdir/forbidden.log"
  if gate_arcade_entry_trace self-forbidden "$tmpdir/good.tsv" "$tmpdir/forbidden.log" 350 50 >/dev/null 2>&1; then
    echo "self-test expected forbidden event failure" >&2
    exit 1
  fi
  printf 'startup_timing\tcatalog_system_navigation_load\t100ms\tstatus=ready\n' >"$tmpdir/forbidden.log"
  if gate_arcade_entry_trace self-forbidden-system "$tmpdir/good.tsv" "$tmpdir/forbidden.log" 350 50 >/dev/null 2>&1; then
    echo "self-test expected forbidden system event failure" >&2
    exit 1
  fi
  echo "profile-arcade-scroll self-test ok"
}

run_boot_prelude() {
  echo "==> Boot prelude: Home -> Arcade entry open_gate=${entry_open_gate_ms}ms interactive_gate=${entry_gate_ms}ms label=$label"
  echo "==> Refresh warm catalog projections before measured reboot"
  "$MISTER" run "/media/fat/mister-magik/mister-magik-fb repair-catalog-projections" >/dev/null
  {
    printf 'export MISTER_CATALOG_REFRESH=default\n'
    printf 'export MISTER_LAUNCHER_START_SCREEN=home\n'
    printf 'export MISTER_HOME_SELECTED_INDEX=%q\n' "$home_selected_index"
    printf 'export MISTER_LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES=1\n'
    printf 'export MISTER_LAUNCHER_INPUT_SCRIPT=%q\n' 'a,down,down,down,down,down,down,down,down'
    printf 'export MISTER_ARCADE_ENTRY_TRACE=%q\n' "$remote_entry_tsv"
  } >"$env_file"
  "$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
  "$MISTER" run "rm -f '$remote_entry_tsv' '$REMOTE_LOG'; sync" >/dev/null
  "$MISTER" reboot-wait
  local waited=0
  while [[ "$waited" -le 45 ]]; do
    if "$MISTER" run "test -s '$remote_entry_tsv' && grep -q '^arcade_first_nav_presented	' '$remote_entry_tsv' && grep -q '^arcade_preview_exact	' '$remote_entry_tsv'" >/dev/null 2>&1; then
      break
    fi
    sleep 1
    waited=$((waited + 1))
  done
  "$MISTER" get "$remote_entry_tsv" "$local_entry_tsv" >/dev/null || true
  "$MISTER" get "$REMOTE_LOG" "$local_entry_log" >/dev/null || true
  echo "wrote $local_entry_tsv"
  echo "wrote $local_entry_log"
  gate_arcade_entry_trace "$label" "$local_entry_tsv" "$local_entry_log" "$entry_open_gate_ms" "$entry_gate_ms"
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

if [[ "$self_test" == "1" ]]; then
  run_arcade_entry_self_test
  exit 0
fi

case "$deploy" in
  device) "$HERE/scripts/deploy-rust.sh" --device --ui-scope launcher --bench-tools ;;
  skip) : ;;
esac

if [[ "$boot_prelude" != "0" ]]; then
  run_boot_prelude
fi

echo "==> Capture supervised launcher Arcade scenario=$scenario remote_scenario=$remote_scenario secs=$secs label=$label deploy=$deploy ui_fb_size=$ui_fb_size present_delay_us=$present_delay_us stream_consumer=$stream_consumer"
{
  printf 'export MISTER_UI_FB_SIZE=%q\n' "$ui_fb_size"
  printf 'export MISTER_FB_PRESENT_DELAY_US=%q\n' "$present_delay_us"
  printf 'export MISTER_CATALOG_REFRESH=default\n'
  printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
  printf 'export MISTER_LAUNCHER_START_SYSTEM=arcade\n'
  printf 'export MISTER_LAUNCHER_LOCK_SCREEN=arcade\n'
  printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=%q\n' "$remote_scenario"
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
echo
check_preview_exact_gate "$label" "$local_tsv"
