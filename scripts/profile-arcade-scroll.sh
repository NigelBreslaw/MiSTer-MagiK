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
Usage: scripts/profile-arcade-scroll.sh [LABEL] [--secs N] [--scenario held-scroll|turbo-hold|human-turbo-hold|velocity-scroll] [--skip-build|--deploy-device] [--present-backend fpga-vblank-latch-hidden|fb0-dirty] [--cpu-profile] [--thread-sample] [--skip-boot-prelude] [--entry-open-gate-ms N] [--entry-gate-ms N] [--selection-invert on|off] [--ui-fb-size auto|960x540|1280x720] [--present-delay-us N] [--catalog-refresh default|off|force] [--stream-consumer none|desktop-bench|desktop-display|null-drain] [--stream-secs N] [--stream-scale off|full|half|adaptive] [--frame-pacing-policy auto|strict|vsync-integrity] [--self-test]

Legacy positional form is still accepted:
  scripts/profile-arcade-scroll.sh [SECS] [LABEL]

Runs the Main-supervised launcher on the real Arcade screen with
MISTER_LAUNCHER_BENCH_SCENARIO and MISTER_PREVIEW_SCROLL_TRACE. By default it
reboots to Home, quickly navigates to the Arcade tile, enters it, then starts
the timed turbo scroll trace in that same launcher session.
Use turbo-hold or human-turbo-hold for Arcade list/preview latch claims and
active-Arcade frame-tail/status-write claims. Report the trace metric owned by
the change, plus passive latch evidence from the generated
*-arcade-latch-drops.tsv and *-fpga-latch-before/after.log artifacts. Use the
Home latch gate instead for Home render/pan or normal launcher copy-path claims.
Requires a deployed bench-tools MagiK binary; --deploy-device builds one.
--cpu-profile builds/deploys the profiling binary, runs the same boot-entry
Arcade scenario with MISTER_PPROF=1, exits after the trace window, and pulls a
non-empty CPU SVG artifact.
--skip-boot-prelude keeps the old direct-to-Arcade benchmark setup.
Set MISTER_ARCADE_ENTRY_INPUT_SCRIPT to override the Home-to-Arcade input
sequence. By default the script presses Right MISTER_ARCADE_ENTRY_HOME_SELECTED_INDEX
times, then A.
--self-test runs only the host parser checks for the boot prelude gate.
--thread-sample records /proc per-thread CPU/core/scheduler samples once per
second while the timed scenario runs.
--selection-invert on|off toggles selected-row inversion for A/B cost runs.
--stream-consumer starts a desktop framebuffer stream consumer during the
timed window. desktop-bench decodes/RGBA-converts frames, desktop-display runs
the real Analytics render path, and null-drain reads without image conversion.

Do not use row-step `list-scroll` for arcade performance benchmarking. It does
not reproduce real velocity scrolling.

Default: --skip-build, useful when the desired binary is already deployed.
EOF
}

secs="30"
label="arcade-scroll-$(date -u +%Y%m%dT%H%M%SZ)"
scenario="turbo-hold"
human_turbo_idle_frames="${MISTER_HUMAN_TURBO_IDLE_FRAMES:-30}"
human_turbo_normal_frames="${MISTER_HUMAN_TURBO_NORMAL_FRAMES:-30}"
human_turbo_pause_frames="${MISTER_HUMAN_TURBO_PAUSE_FRAMES:-30}"
entry_before_a_wait_frames="${MISTER_ARCADE_ENTRY_BEFORE_A_WAIT_FRAMES:-12}"
repair_projections="${MISTER_ARCADE_SCROLL_REPAIR_PROJECTIONS:-0}"
catalog_refresh="${MISTER_CATALOG_REFRESH:-default}"
deploy="skip"
selection_invert=""
ui_fb_size="${MISTER_UI_FB_SIZE:-auto}"
present_delay_us="${MISTER_FB_PRESENT_DELAY_US:-0}"
stream_consumer="${MISTER_FRAMEBUFFER_STREAM_CONSUMER:-none}"
stream_secs=""
stream_scale="${MISTER_FRAMEBUFFER_STREAM_SCALE:-off}"
present_backend="${MISTER_PRESENT_BACKEND:-fpga-vblank-latch-hidden}"
cpu_profile="0"
cpu_profile_remote_svg=""
boot_prelude="${MISTER_ARCADE_SCROLL_BOOT_PRELUDE:-1}"
entry_open_gate_ms="${MISTER_ARCADE_ENTRY_OPEN_GATE_MS:-2000}"
entry_gate_ms="${MISTER_ARCADE_ENTRY_GATE_MS:-100}"
home_selected_index="${MISTER_ARCADE_ENTRY_HOME_SELECTED_INDEX:-7}"
entry_input_script="${MISTER_ARCADE_ENTRY_INPUT_SCRIPT:-}"
frame_pacing_p99_work_us="${MISTER_ARCADE_SCROLL_P99_WORK_US:-14500}"
frame_pacing_p99_wall_us="${MISTER_ARCADE_SCROLL_P99_WALL_US:-16000}"
frame_pacing_max_wall_us="${MISTER_ARCADE_SCROLL_MAX_WALL_US:-16667}"
frame_pacing_policy="auto"
self_test="0"
positionals=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) deploy="skip"; shift ;;
    --deploy-device) deploy="device"; shift ;;
    --cpu-profile) cpu_profile="1"; shift ;;
    --thread-sample) thread_sample_enabled="1"; shift ;;
    --present-backend)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--present-backend needs fpga-vblank-latch-hidden or fb0-dirty" >&2; usage >&2; exit 2; fi
      present_backend="$2"
      shift 2
      ;;
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
    --catalog-refresh)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--catalog-refresh needs default, off, or force" >&2; usage >&2; exit 2; fi
      catalog_refresh="$2"
      shift 2
      ;;
    --stream-consumer)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--stream-consumer needs none, desktop-bench, desktop-display, or null-drain" >&2; usage >&2; exit 2; fi
      stream_consumer="$2"
      shift 2
      ;;
    --stream-secs)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--stream-secs needs a value" >&2; usage >&2; exit 2; fi
      stream_secs="$2"
      shift 2
      ;;
    --stream-scale)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--stream-scale needs off, full, half, or adaptive" >&2; usage >&2; exit 2; fi
      stream_scale="$2"
      shift 2
      ;;
    --frame-pacing-policy)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--frame-pacing-policy needs auto, strict, or vsync-integrity" >&2; usage >&2; exit 2; fi
      frame_pacing_policy="$2"
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
if [[ -n "$stream_secs" && ! "$stream_secs" =~ ^[0-9]+$ ]]; then echo "--stream-secs must be an integer number of seconds" >&2; exit 2; fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then echo "label must contain only letters, numbers, _, ., or -" >&2; exit 2; fi
if [[ ! "$entry_open_gate_ms" =~ ^[0-9]+$ ]]; then echo "--entry-open-gate-ms must be an integer" >&2; exit 2; fi
if [[ ! "$entry_gate_ms" =~ ^[0-9]+$ ]]; then echo "--entry-gate-ms must be an integer" >&2; exit 2; fi
if [[ ! "$home_selected_index" =~ ^[0-9]+$ ]]; then echo "MISTER_ARCADE_ENTRY_HOME_SELECTED_INDEX must be an integer" >&2; exit 2; fi
case "$present_backend" in
  fpga-vblank-latch-hidden|fb0-dirty) ;;
  *) echo "--present-backend must be fpga-vblank-latch-hidden or fb0-dirty" >&2; exit 2 ;;
esac
if [[ ! "$human_turbo_idle_frames" =~ ^[0-9]+$ ]]; then echo "MISTER_HUMAN_TURBO_IDLE_FRAMES must be an integer" >&2; exit 2; fi
if [[ ! "$human_turbo_normal_frames" =~ ^[0-9]+$ ]]; then echo "MISTER_HUMAN_TURBO_NORMAL_FRAMES must be an integer" >&2; exit 2; fi
if [[ ! "$human_turbo_pause_frames" =~ ^[0-9]+$ ]]; then echo "MISTER_HUMAN_TURBO_PAUSE_FRAMES must be an integer" >&2; exit 2; fi
if [[ ! "$entry_before_a_wait_frames" =~ ^[0-9]+$ ]]; then echo "MISTER_ARCADE_ENTRY_BEFORE_A_WAIT_FRAMES must be an integer" >&2; exit 2; fi
if [[ ! "$frame_pacing_p99_work_us" =~ ^[0-9]+$ ]]; then echo "MISTER_ARCADE_SCROLL_P99_WORK_US must be an integer" >&2; exit 2; fi
if [[ ! "$frame_pacing_p99_wall_us" =~ ^[0-9]+$ ]]; then echo "MISTER_ARCADE_SCROLL_P99_WALL_US must be an integer" >&2; exit 2; fi
if [[ ! "$frame_pacing_max_wall_us" =~ ^[0-9]+$ ]]; then echo "MISTER_ARCADE_SCROLL_MAX_WALL_US must be an integer" >&2; exit 2; fi
case "$scenario" in
  velocity-scroll|held-scroll|turbo-hold|human-turbo-hold) ;;
  list-scroll|smooth-scroll|selected-first|stress-scroll|cache-warm|preview|preview-changes|screenshot-stress|preview-stress)
    echo "row-step/jump scenario '$scenario' is not valid for arcade benchmarking; use velocity-scroll, held-scroll, turbo-hold, or human-turbo-hold" >&2
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
  none|desktop-bench|desktop-display|null-drain) ;;
  *) echo "--stream-consumer must be none, desktop-bench, desktop-display, or null-drain" >&2; exit 2 ;;
esac
case "$stream_scale" in
  off|full|half|adaptive) ;;
  *) echo "--stream-scale must be off, full, half, or adaptive" >&2; exit 2 ;;
esac
case "$frame_pacing_policy" in
  auto|strict|vsync-integrity) ;;
  *) echo "--frame-pacing-policy must be auto, strict, or vsync-integrity" >&2; exit 2 ;;
esac
case "$catalog_refresh" in
  default|off|force) ;;
  *) echo "--catalog-refresh must be default, off, or force" >&2; exit 2 ;;
esac
remote_scenario="$scenario"
if [[ "$remote_scenario" == "velocity-scroll" ]]; then remote_scenario="held-scroll"; fi

mkdir -p "$OUT_DIR"
remote_tsv="/tmp/${label}-arcade-scroll.tsv"
remote_log="$REMOTE_LOG"
local_tsv="$OUT_DIR/${label}-arcade-scroll.tsv"
local_log="$OUT_DIR/${label}-arcade-scroll.log"
local_status_json="$OUT_DIR/${label}-arcade-scroll.status.json"
local_cpu_svg="$OUT_DIR/${label}-arcade-scroll-cpu.svg"
local_stream_tsv="$OUT_DIR/${label}-framebuffer-stream.tsv"
local_stream_log="$OUT_DIR/${label}-framebuffer-stream.log"
local_cadence_tsv="$OUT_DIR/${label}-framebuffer-cadence.tsv"
local_latch_before="$OUT_DIR/${label}-fpga-latch-before.log"
local_latch_after="$OUT_DIR/${label}-fpga-latch-after.log"
local_latch_drop_report="$OUT_DIR/${label}-arcade-latch-drops.tsv"
remote_entry_tsv="/tmp/${label}-arcade-entry.tsv"
local_entry_tsv="$OUT_DIR/${label}-arcade-entry.tsv"
local_entry_log="$OUT_DIR/${label}-arcade-entry.log"
env_file="$(mktemp "${TMPDIR:-/tmp}/mister-magik-arcade-scroll-env.XXXXXX")"
run_id="${label}-$(date -u +%Y%m%dT%H%M%SZ)-$$"
stream_pid=""
stream_seconds="${stream_secs:-$secs}"
present_width="960"
if [[ "$ui_fb_size" == "1280x720" ]]; then
  present_width="1280"
fi
latch_reports_enabled="0"
if [[ "$present_backend" == "fpga-vblank-latch-hidden" ]]; then
  latch_reports_enabled="1"
fi

capture_latch_report() {
  local phase="$1" out="$2"
  if [[ "$latch_reports_enabled" != "1" ]]; then
    return 0
  fi
  if "$MISTER" run "'/media/fat/mister-magik/mister-magik-fb' fpga-latch-report" >"$out"; then
    echo "wrote $out"
    return 0
  fi
  echo "failed to capture FPGA latch report phase=$phase path=$out" >&2
  return 1
}

check_composition_recovery_gate() {
  local status_json="$1"
  python3 - "$status_json" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
slint = data.get("runtime", {}).get("slint_status")
if not isinstance(slint, dict):
    print("composition_gate_tsv\tvalid=0\tinvalid_reason=missing_slint_status")
    raise SystemExit(11)
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
missing_columns = [
    column
    for column in ("cache_state", "transition_effect", "transition_progress")
    if column not in rows[0]
]
if missing_columns:
    print(f"preview_exact_gate_tsv\tlabel={name}\tvalid=0\tinvalid_reason=missing_column\tdetail={','.join(missing_columns)}")
    sys.exit(9)

counts = collections.Counter(row.get("cache_state", "") for row in rows)
invalid_preview = [
    row for row in rows
    if row.get("cache_state") not in ("exact", "empty")
]
detail = " ".join(
    f"{state or 'blank'}={count}"
    for state, count in sorted(counts.items())
)
fade_rows = []
for row in rows:
    try:
        progress = float(row.get("transition_progress", "") or "1")
    except ValueError:
        progress = 1.0
    if row.get("transition_effect") == "fade" and 0.0 < progress < 1.0:
        fade_rows.append(row)

if invalid_preview:
    samples = []
    for row in invalid_preview[:10]:
        samples.append(
            f"frame={row.get('frame', '?')}:selected={row.get('selected', '?')}:cache_state={row.get('cache_state', '') or 'blank'}"
        )
    print(
        f"preview_exact_gate_tsv\tlabel={name}\tvalid=0\tinvalid_reason=non_exact_preview"
        f"\tdetail=frames={len(rows)} exact={counts.get('exact', 0)} empty={counts.get('empty', 0)} invalid={len(invalid_preview)} fade_rows={len(fade_rows)} {detail}"
        f"\tsamples={','.join(samples)}"
    )
    sys.exit(9)

if len(fade_rows) < 2:
    samples = ",".join(
        f"frame={row.get('frame', '?')}:progress={row.get('transition_progress', '?')}"
        for row in fade_rows[:10]
    )
    print(
        f"preview_exact_gate_tsv\tlabel={name}\tvalid=0\tinvalid_reason=missing_fade_transition"
        f"\tdetail=frames={len(rows)} exact={counts.get('exact', 0)} empty={counts.get('empty', 0)} fade_rows={len(fade_rows)} {detail}"
        f"\tsamples={samples}"
    )
    sys.exit(9)

print(
    f"preview_exact_gate_tsv\tlabel={name}\tvalid=1\tinvalid_reason=ok"
    f"\tdetail=frames={len(rows)} exact={counts.get('exact', 0)} empty={counts.get('empty', 0)} invalid=0 fade_rows={len(fade_rows)} {detail}"
)
PY
}

check_frame_pacing_gate() {
  local name="$1" trace="$2" p99_work_us="$3" p99_wall_us="$4" max_wall_us="$5" gate_scenario="${6:-}" policy="${7:-auto}"
  "$HERE/scripts/check-frame-pacing-trace.py" "$name" "$trace" "$p99_work_us" "$p99_wall_us" "$max_wall_us" "$gate_scenario" "$policy"
}

gate_arcade_entry_trace() {
  local name="$1" trace="$2" log="$3" open_threshold="$4" interactive_threshold="$5" expected_run_id="${6:-}"
  python3 - "$name" "$trace" "$log" "$open_threshold" "$interactive_threshold" "$expected_run_id" <<'PY'
import csv
import re
import sys

name, trace_path, log_path, open_threshold_s, interactive_threshold_s, expected_run_id = sys.argv[1:7]
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

if expected_run_id:
    mismatched = [event for event in required if rows[event].get("run_id") != expected_run_id]
    if mismatched:
        detail = ",".join(
            f"{event}:{rows[event].get('run_id', '<missing>')}" for event in mismatched
        )
        print(f"arcade_entry_gate_tsv\tlabel={name}\tvalid=0\tinvalid_reason=run_id_mismatch\tdetail=expected={expected_run_id} actual={detail}")
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
  awk 'BEGIN { FS=OFS="\t" } NR == 1 { print $1, "run_id", $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12; next } { print $1, "run-1", $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12 }' "$tmpdir/good.tsv" >"$tmpdir/good-run.tsv"
  gate_arcade_entry_trace self-good-run "$tmpdir/good-run.tsv" "$tmpdir/good.log" 350 50 "run-1" >/dev/null
  if gate_arcade_entry_trace self-wrong-run "$tmpdir/good-run.tsv" "$tmpdir/good.log" 350 50 "run-2" >/dev/null 2>&1; then
    echo "self-test expected run id mismatch failure" >&2
    exit 1
  fi
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
frame	selected	cache_state	transition_effect	transition_progress
0	0	exact	fade	0.25
1	1	exact	fade	0.75
EOF
  check_preview_exact_gate self-exact "$tmpdir/exact-scroll.tsv" >/dev/null
  cat >"$tmpdir/stale-scroll.tsv" <<'EOF'
frame	selected	cache_state	transition_effect	transition_progress
0	0	exact	fade	0.25
1	1	stale	fade	0.75
EOF
  if check_preview_exact_gate self-stale "$tmpdir/stale-scroll.tsv" >/dev/null 2>&1; then
    echo "self-test expected stale preview failure" >&2
    exit 1
  fi
  cat >"$tmpdir/cut-scroll.tsv" <<'EOF'
frame	selected	cache_state	transition_effect	transition_progress
0	0	exact	fade	1
1	1	exact	fade	1
EOF
  if check_preview_exact_gate self-cut "$tmpdir/cut-scroll.tsv" >/dev/null 2>&1; then
    echo "self-test expected missing fade failure" >&2
    exit 1
  fi
  cat >"$tmpdir/pacing-good.tsv" <<'EOF'
frame	wall_us	prepare_us	slint_render_us	custom_draw_us	fb_present_us	vsync_source	vsync_miss_streak
31	400	100	100	100	100	none	0
32	15000	1000	200	1200	900	vsync	0
33	15500	1100	200	1200	900	vsync	0
EOF
  check_frame_pacing_gate self-pacing "$tmpdir/pacing-good.tsv" 14500 16000 16667 >/dev/null
  cat >"$tmpdir/pacing-fallback.tsv" <<'EOF'
frame	wall_us	prepare_us	slint_render_us	custom_draw_us	fb_present_us	vsync_source	vsync_miss_streak
31	15000	1000	200	1200	900	vsync	0
32	15000	1000	200	1200	900	fallback	1
EOF
  if check_frame_pacing_gate self-pacing "$tmpdir/pacing-fallback.tsv" 14500 16000 16667 >/dev/null 2>&1; then
    echo "self-test expected frame pacing failure" >&2
    exit 1
  fi
  cat >"$tmpdir/pacing-wall.tsv" <<'EOF'
frame	wall_us	prepare_us	slint_render_us	custom_draw_us	fb_present_us	vsync_source	vsync_miss_streak
31	15000	1000	200	1200	900	vsync	0
32	16668	1000	200	1200	900	vsync	0
EOF
  if check_frame_pacing_gate self-pacing "$tmpdir/pacing-wall.tsv" 14500 16000 16667 >/dev/null 2>&1; then
    echo "self-test expected wall pacing failure" >&2
    exit 1
  fi
  check_frame_pacing_gate self-human-turbo-wall "$tmpdir/pacing-wall.tsv" 14500 16000 16667 human-turbo-hold >/dev/null
  check_frame_pacing_gate self-stream-turbo-wall "$tmpdir/pacing-wall.tsv" 14500 16000 16667 turbo-hold vsync-integrity >/dev/null
  cat >"$tmpdir/pacing-human-turbo-wall.tsv" <<'EOF'
frame	wall_us	prepare_us	slint_render_us	custom_draw_us	fb_present_us	vsync_source	vsync_miss_streak
31	15000	1000	200	1200	900	vsync	0
32	20001	1000	200	1200	900	vsync	0
EOF
  check_frame_pacing_gate self-human-turbo-wall-20ms "$tmpdir/pacing-human-turbo-wall.tsv" 14500 16000 16667 human-turbo-hold >/dev/null
  cat >"$tmpdir/pacing-human-turbo-wall-33ms.tsv" <<'EOF'
frame	wall_us	prepare_us	slint_render_us	custom_draw_us	fb_present_us	vsync_source	vsync_miss_streak
31	15000	1000	200	1200	900	vsync	0
32	33335	1000	200	1200	900	vsync	0
EOF
  if check_frame_pacing_gate self-human-turbo-wall-fail "$tmpdir/pacing-human-turbo-wall-33ms.tsv" 14500 16000 16667 human-turbo-hold >/dev/null 2>&1; then
    echo "self-test expected human-turbo >33ms wall pacing failure" >&2
    exit 1
  fi
  cat >"$tmpdir/pacing-missing-column.tsv" <<'EOF'
frame	wall_us	prepare_us	slint_render_us	custom_draw_us	fb_present_us	vsync_source
31	15000	1000	200	1200	900	vsync
EOF
  if check_frame_pacing_gate self-pacing "$tmpdir/pacing-missing-column.tsv" 14500 16000 16667 >/dev/null 2>&1; then
    echo "self-test expected missing frame pacing column failure" >&2
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
  echo "==> Boot flow: Home -> Arcade -> ${scenario} scroll open_gate=${entry_open_gate_ms}ms interactive_gate=${entry_gate_ms}ms label=$label"
  if [[ "$repair_projections" == "1" || "$repair_projections" == "true" || "$repair_projections" == "yes" ]]; then
    echo "==> Refresh warm catalog projections before measured reboot"
    "$MISTER" run "/media/fat/mister-magik/mister-magik-fb repair-catalog-projections" >/dev/null
  fi
  local input_script="$entry_input_script"
  if [[ -z "$input_script" ]]; then
    input_script=""
    for ((i = 0; i < home_selected_index; i++)); do
      if [[ -n "$input_script" ]]; then input_script+=","; fi
      input_script+="right"
    done
    if [[ "$entry_before_a_wait_frames" -gt 0 ]]; then
      if [[ -n "$input_script" ]]; then input_script+=","; fi
      input_script+="wait:$entry_before_a_wait_frames"
    fi
    if [[ -n "$input_script" ]]; then input_script+=","; fi
    input_script+="a"
  fi
  {
    printf 'export MISTER_UI_FB_SIZE=%q\n' "$ui_fb_size"
    printf 'export MISTER_FB_PRESENT_DELAY_US=%q\n' "$present_delay_us"
    printf 'export MISTER_CATALOG_REFRESH=%q\n' "$catalog_refresh"
    printf 'export MISTER_PRESENT_BACKEND=%q\n' "$present_backend"
    printf 'export MISTER_FRAMEBUFFER_STREAM_SCALE=%q\n' "$stream_scale"
    printf 'export MISTER_LAUNCHER_START_SCREEN=home\n'
    printf 'export MISTER_LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES=1\n'
    printf 'export MISTER_LAUNCHER_INPUT_SCRIPT=%q\n' "$input_script"
    printf 'export MISTER_LAUNCHER_BENCH_AFTER_INPUT_SCRIPT=1\n'
    printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=%q\n' "$remote_scenario"
    printf 'export MISTER_HUMAN_TURBO_IDLE_FRAMES=%q\n' "$human_turbo_idle_frames"
    printf 'export MISTER_HUMAN_TURBO_NORMAL_FRAMES=%q\n' "$human_turbo_normal_frames"
    printf 'export MISTER_HUMAN_TURBO_PAUSE_FRAMES=%q\n' "$human_turbo_pause_frames"
    printf 'export MISTER_PREVIEW_SCROLL_TRACE_SECS=%q\n' "$secs"
    printf 'export MISTER_PREVIEW_SCROLL_TRACE=%q\n' "$remote_tsv"
    if [[ "$cpu_profile" == "1" ]]; then
      cpu_profile_remote_svg="/tmp/${label}-arcade-scroll-cpu.svg"
      printf 'export MISTER_PPROF=1\n'
      printf 'export MISTER_PPROF_OUT=%q\n' "$cpu_profile_remote_svg"
      printf 'export MISTER_PREVIEW_SCROLL_EXIT_AFTER_TRACE=1\n'
    fi
    printf 'export MISTER_ARCADE_ENTRY_TRACE=%q\n' "$remote_entry_tsv"
    printf 'export MISTER_ARCADE_ENTRY_RUN_ID=%q\n' "$run_id"
    if [[ "$selection_invert" == "off" ]]; then
      printf 'export MISTER_ARCADE_SELECTION_INVERT=0\n'
    elif [[ "$selection_invert" == "on" ]]; then
      printf 'export MISTER_ARCADE_SELECTION_INVERT=1\n'
    fi
    if [[ -n "${MISTER_PREVIEW_TURBO_RUNWAY+x}" ]]; then
      printf 'export MISTER_PREVIEW_TURBO_RUNWAY=%q\n' "$MISTER_PREVIEW_TURBO_RUNWAY"
    fi
    if [[ -n "${MISTER_PREVIEW_TURBO_LOOKAHEAD+x}" ]]; then
      printf 'export MISTER_PREVIEW_TURBO_LOOKAHEAD=%q\n' "$MISTER_PREVIEW_TURBO_LOOKAHEAD"
    fi
  } >"$env_file"
  rm -f "$local_tsv" "$local_log" "$local_status_json" "$local_entry_tsv" "$local_entry_log" "$local_cpu_svg" "$local_stream_tsv" "$local_stream_log" "$local_cadence_tsv" "$local_latch_before" "$local_latch_after" "$local_latch_drop_report"
  "$MISTER" run "rm -f '$REMOTE_ENV' '$remote_entry_tsv' '$remote_tsv' '$REMOTE_LOG' '$cpu_profile_remote_svg'; sync" >/dev/null
  "$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
  echo "==> Armed fresh boot-entry run_id=$run_id entry_before_a_wait_frames=$entry_before_a_wait_frames human_idle_frames=$human_turbo_idle_frames human_normal_frames=$human_turbo_normal_frames human_pause_frames=$human_turbo_pause_frames"
  "$MISTER" reboot-wait
  capture_latch_report before "$local_latch_before"
  start_stream_consumer
  thread_sample_start "$label" "arcade-scroll" "$OUT_DIR" $((secs + 10))
  sleep $((secs + 10))
  thread_sample_finish
  stream_status=0
  finish_stream_consumer || stream_status="$?"
  local waited=0
  while [[ "$waited" -le 15 ]]; do
    if "$MISTER" run "test -s '$remote_entry_tsv' && test -s '$remote_tsv' && grep -q '^arcade_first_nav_presented	' '$remote_entry_tsv' && grep -q '^arcade_preview_exact	' '$remote_entry_tsv'" >/dev/null 2>&1; then
      break
    fi
    echo "==> Waiting for boot-entry trace artifacts (${waited}s) label=$label run_id=$run_id" >&2
    sleep 1
    waited=$((waited + 1))
  done
  if ! "$MISTER" get "$remote_tsv" "$local_tsv" >/dev/null; then
    "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null || true
    echo "arcade scroll profile failed; see $local_log" >&2
    exit 1
  fi
  "$MISTER" get "$remote_entry_tsv" "$local_entry_tsv" >/dev/null || true
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null || true
  cp "$local_log" "$local_entry_log" 2>/dev/null || true
  "$MISTER" status --json >"$local_status_json" 2>/dev/null || true
  capture_latch_report after "$local_latch_after"
  if [[ "$cpu_profile" == "1" ]]; then
    if ! "$MISTER" get "$cpu_profile_remote_svg" "$local_cpu_svg" >/dev/null || [[ ! -s "$local_cpu_svg" ]]; then
      echo "arcade scroll CPU profile failed or produced an empty SVG; see $local_log" >&2
      exit 9
    fi
    if ! grep -q 'cpu_profile:' "$local_log"; then
      echo "arcade scroll CPU profile log does not contain cpu_profile output; see $local_log" >&2
      exit 9
    fi
    echo "wrote $local_cpu_svg"
  fi
  echo "wrote $local_tsv"
  echo "wrote $local_log"
  echo "wrote $local_status_json"
  echo "wrote $local_entry_tsv"
  gate_arcade_entry_trace "$label" "$local_entry_tsv" "$local_entry_log" "$entry_open_gate_ms" "$entry_gate_ms" "$run_id"
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
}

start_stream_consumer() {
  stream_features=(--release)
  case "$stream_consumer" in
    none) return 0 ;;
    desktop-bench) stream_arg="--framebuffer-stream-bench-secs" ;;
    desktop-display)
      stream_arg="--framebuffer-stream-display-bench-secs"
      stream_features+=(--no-default-features --features compiled-ui,skia-renderer)
      ;;
    null-drain) stream_arg="--framebuffer-stream-drain-bench-secs" ;;
  esac
  echo "==> Start framebuffer stream consumer mode=$stream_consumer seconds=$stream_seconds scale=$stream_scale"
  (
    cd "$HERE"
    MISTER_IP="${MISTER_IP:-192.168.1.117}" MISTER_FRAMEBUFFER_CADENCE_OUT="$local_cadence_tsv" cargo run --manifest-path desktop/Cargo.toml --locked "${stream_features[@]}" -- "$stream_arg" "$stream_seconds"
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
    printf 'framebuffer_stream_bench_tsv\tmode=%s\tseconds=%s\tcompleted=0\tinvalid_reason=consumer_timeout\n' \
      "$stream_consumer" "$stream_seconds" | tee -a "$local_stream_tsv"
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

if [[ "$cpu_profile" == "1" && "$self_test" != "1" ]]; then
  profile_bin="$HERE/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device-profile/mister-magik-fb"
  echo "==> Build profiling binary for boot-entry Arcade CPU profile"
  "$HERE/magik-gui/build-arm.sh" --profile --ui-scope launcher --bench-tools
  echo "==> Deploy profiling binary for boot-entry Arcade CPU profile"
  if ! "$MISTER" agent deploy-magik-bin "$profile_bin" /media/fat/mister-magik/mister-magik-fb >/dev/null; then
    echo "agent deploy failed for profiling binary; falling back to device deploy transaction" >&2
    "$MISTER" deploy-magik-bin "$profile_bin" /media/fat/mister-magik/mister-magik-fb >/dev/null
  fi
fi

if [[ "$boot_prelude" != "0" ]]; then
  run_boot_prelude
else
  echo "==> Capture supervised launcher Arcade scenario=$scenario remote_scenario=$remote_scenario secs=$secs label=$label deploy=$deploy ui_fb_size=$ui_fb_size present_delay_us=$present_delay_us stream_consumer=$stream_consumer"
  {
    printf 'export MISTER_UI_FB_SIZE=%q\n' "$ui_fb_size"
    printf 'export MISTER_FB_PRESENT_DELAY_US=%q\n' "$present_delay_us"
    printf 'export MISTER_CATALOG_REFRESH=%q\n' "$catalog_refresh"
    printf 'export MISTER_PRESENT_BACKEND=%q\n' "$present_backend"
    printf 'export MISTER_FRAMEBUFFER_STREAM_SCALE=%q\n' "$stream_scale"
    printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_START_SYSTEM=arcade\n'
    printf 'export MISTER_LAUNCHER_LOCK_SCREEN=arcade\n'
    printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=%q\n' "$remote_scenario"
    printf 'export MISTER_HUMAN_TURBO_IDLE_FRAMES=%q\n' "$human_turbo_idle_frames"
    printf 'export MISTER_HUMAN_TURBO_NORMAL_FRAMES=%q\n' "$human_turbo_normal_frames"
    printf 'export MISTER_HUMAN_TURBO_PAUSE_FRAMES=%q\n' "$human_turbo_pause_frames"
    printf 'export MISTER_PREVIEW_SCROLL_TRACE_SECS=%q\n' "$secs"
    printf 'export MISTER_PREVIEW_SCROLL_TRACE=%q\n' "$remote_tsv"
    if [[ "$selection_invert" == "off" ]]; then
      printf 'export MISTER_ARCADE_SELECTION_INVERT=0\n'
    elif [[ "$selection_invert" == "on" ]]; then
      printf 'export MISTER_ARCADE_SELECTION_INVERT=1\n'
    fi
    if [[ -n "${MISTER_PREVIEW_TURBO_RUNWAY+x}" ]]; then
      printf 'export MISTER_PREVIEW_TURBO_RUNWAY=%q\n' "$MISTER_PREVIEW_TURBO_RUNWAY"
    fi
    if [[ -n "${MISTER_PREVIEW_TURBO_LOOKAHEAD+x}" ]]; then
      printf 'export MISTER_PREVIEW_TURBO_LOOKAHEAD=%q\n' "$MISTER_PREVIEW_TURBO_LOOKAHEAD"
    fi
  } >"$env_file"
  rm -f "$local_tsv" "$local_log" "$local_status_json" "$local_entry_tsv" "$local_entry_log" "$local_cpu_svg" "$local_stream_tsv" "$local_stream_log" "$local_cadence_tsv" "$local_latch_before" "$local_latch_after" "$local_latch_drop_report"
  capture_latch_report before "$local_latch_before"
  "$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
  "$MISTER" run "rm -f '$remote_tsv' '$remote_log' '$cpu_profile_remote_svg'; if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd" >/dev/null
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
  capture_latch_report after "$local_latch_after"
  if [[ "$cpu_profile" == "1" ]]; then
    if ! "$MISTER" get "$cpu_profile_remote_svg" "$local_cpu_svg" >/dev/null || [[ ! -s "$local_cpu_svg" ]]; then
      echo "arcade scroll CPU profile failed or produced an empty SVG; see $local_log" >&2
      exit 9
    fi
    if ! grep -q 'cpu_profile:' "$local_log"; then
      echo "arcade scroll CPU profile log does not contain cpu_profile output; see $local_log" >&2
      exit 9
    fi
    echo "wrote $local_cpu_svg"
  fi

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
fi
if [[ -s "$local_status_json" ]] && ! check_composition_recovery_gate "$local_status_json"; then
  echo "arcade scroll composition recovery occurred; see $local_status_json" >&2
  exit 13
fi
echo
"$HERE/scripts/analyze-arcade-frame-trace.py" "$local_tsv" --status-json "$local_status_json"
echo
"$HERE/scripts/launcher-present-trace.py" summarize "$local_tsv" --case arcade-scroll --present-width "$present_width" --ignore-frames-through 30
echo
analyze_args=(--label "$label" --status-json "$local_status_json" --ignore-elapsed-zero)
analyze_args+=(--expect-backend "$present_backend")
if [[ "$latch_reports_enabled" == "1" ]]; then
  analyze_args+=(--fpga-latch-report-before "$local_latch_before")
  analyze_args+=(--fpga-latch-report-after "$local_latch_after")
fi
set +e
"$HERE/scripts/analyze-max-scroll-drops.py" "$local_tsv" "${analyze_args[@]}" | tee "$local_latch_drop_report"
latch_drop_status=${PIPESTATUS[0]}
set -e
echo "wrote $local_latch_drop_report"
if [[ "$latch_drop_status" -ne 0 ]]; then
  echo "arcade latch drop analysis failed; see $local_latch_drop_report" >&2
  exit "$latch_drop_status"
fi
echo
check_frame_pacing_gate "$label" "$local_tsv" "$frame_pacing_p99_work_us" "$frame_pacing_p99_wall_us" "$frame_pacing_max_wall_us" "$scenario" "$frame_pacing_policy"
echo
check_preview_exact_gate "$label" "$local_tsv"
