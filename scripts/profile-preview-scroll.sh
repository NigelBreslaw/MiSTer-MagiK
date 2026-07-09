#!/usr/bin/env bash
# Run a real launcher Arcade preview-scroll benchmark through MiSTer_MagiK.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/preview-scroll-profiles"
PRESENT_TRACE="$HERE/scripts/launcher-present-trace.py"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"
ORIGINAL_ARGS=("$@")
source "$HERE/scripts/thread-sampler-lib.sh"
source "$HERE/scripts/bench-context-lib.sh"

usage() {
  cat <<'EOF'
Usage: scripts/profile-preview-scroll.sh [SECS] [SCENARIO] [LABEL] [--secs N] [--scenario NAME] [--skip-build|--deploy-device] [--cpu-profile] [--thread-sample] [--self-test] [--visual-captures N] [--start-system ID] [--selected-index N] [--defer-start-system] [--skip-preview-warm] [--fade-mode default|legacy] [--replace-label]

Scenarios: velocity-scroll | held-scroll | turbo-hold | preview-step-hold | preview-idle
Runs the real launcher Arcade screen under Main_MiSTer supervision by writing
/media/fat/mister-magik/launcher.env and sending mister_magik_restart_launcher.
Requires a deployed bench-tools MagiK binary; --deploy-device builds one.

--cpu-profile builds/deploys the profiling binary, runs the same supervised
Arcade scenario with MISTER_PPROF=1, exits after the trace window, and pulls a
non-empty CPU SVG artifact.
--visual-captures captures fixed Arcade indices from the real launcher screen.
--start-system sets MISTER_LAUNCHER_START_SYSTEM for direct system benchmarks.
--selected-index starts Arcade on a specific zero-based row before tracing.
--defer-start-system starts on Home and lets MISTER_LAUNCHER_START_SYSTEM enter
the requested system after navigation rows hydrate.
--skip-preview-warm skips launcher benchmark archive preloading so first-preview
measurement can exercise the .idx + pread fast lane.
--fade-mode legacy disables the RGB565 preview fade fast path for A/B timing.
--thread-sample records /proc per-thread CPU/core/scheduler samples once per
second while the timed scenario runs.
--replace-label removes existing local artifacts for LABEL before running.

Do not use row-step scenarios such as list-scroll/smooth-scroll for Arcade
performance benchmarking. They do not reproduce real velocity scrolling.
EOF
}

secs="30"
scenario="velocity-scroll"
label="preview-scroll-$(date -u +%Y%m%dT%H%M%SZ)"
deploy="skip"
self_test="0"
visual_captures="4"
allow_hotpath_misses="${MISTER_ALLOW_PREVIEW_HOTPATH_MISSES:-0}"
cpu_profile="0"
cpu_profile_remote_svg=""
skip_preview_warm="${MISTER_PREVIEW_SCROLL_SKIP_ARCHIVE_WARM:-0}"
replace_label="0"
selected_index=""
start_system="arcade"
defer_start_system="0"
fade_mode="default"
positionals=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) deploy="skip"; shift ;;
    --deploy-device) deploy="device"; shift ;;
    --cpu-profile) cpu_profile="1"; shift ;;
    --thread-sample) thread_sample_enabled="1"; shift ;;
    --self-test) self_test="1"; shift ;;
    --replace-label) replace_label="1"; shift ;;
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
    --visual-captures) visual_captures="${2:-}"; shift 2 ;;
    --start-system)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--start-system needs a value" >&2; usage >&2; exit 2; fi
      start_system="$2"
      shift 2
      ;;
    --selected-index)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--selected-index needs a value" >&2; usage >&2; exit 2; fi
      selected_index="$2"
      shift 2
      ;;
    --defer-start-system) defer_start_system="1"; shift ;;
    --skip-preview-warm|--cold-preview-load) skip_preview_warm="1"; shift ;;
    --fade-mode)
      if [[ $# -lt 2 || "${2:-}" == --* ]]; then echo "--fade-mode needs a value" >&2; usage >&2; exit 2; fi
      fade_mode="$2"
      shift 2
      ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) positionals+=("$1"); shift ;;
  esac
done

if [[ "${#positionals[@]}" -gt 3 ]]; then usage >&2; exit 2; fi
if [[ "${#positionals[@]}" -ge 1 ]]; then
  if [[ "${positionals[0]}" =~ ^[0-9]+$ ]]; then
    secs="${positionals[0]}"
    if [[ "${#positionals[@]}" -ge 2 ]]; then scenario="${positionals[1]}"; fi
    if [[ "${#positionals[@]}" -ge 3 ]]; then label="${positionals[2]}"; fi
  else
    label="${positionals[0]}"
    if [[ "${#positionals[@]}" -ge 2 ]]; then scenario="${positionals[1]}"; fi
    if [[ "${#positionals[@]}" -ge 3 ]]; then
      echo "unexpected argument after LABEL SCENARIO: ${positionals[2]}" >&2
      usage >&2
      exit 2
    fi
  fi
fi

case "$scenario" in
  velocity-scroll|held-scroll|turbo-hold|preview-step-hold|preview-idle) ;;
  list-scroll|smooth-scroll|selected-first|stress-scroll|cache-warm|preview|preview-changes|screenshot-stress|preview-stress)
    echo "row-step/jump scenario '$scenario' is not valid for preview benchmarking; use velocity-scroll, turbo-hold, preview-step-hold, or preview-idle" >&2
    exit 2
    ;;
  *) echo "unknown scenario: $scenario" >&2; usage >&2; exit 2 ;;
esac
remote_scenario="$scenario"
if [[ "$remote_scenario" == "velocity-scroll" ]]; then remote_scenario="held-scroll"; fi
if [[ ! "$secs" =~ ^[0-9]+$ ]]; then echo "secs must be an integer" >&2; exit 2; fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then echo "label must contain only letters, numbers, _, ., or -" >&2; exit 2; fi
if [[ ! "$visual_captures" =~ ^[0-9]+$ ]]; then echo "--visual-captures must be an integer" >&2; exit 2; fi
if [[ -n "$selected_index" && ! "$selected_index" =~ ^[0-9]+$ ]]; then echo "--selected-index must be an integer" >&2; exit 2; fi
if [[ ! "$start_system" =~ ^[A-Za-z0-9_.-]+$ ]]; then echo "--start-system must contain only letters, numbers, _, ., or -" >&2; exit 2; fi
case "$fade_mode" in default|legacy) ;; *) echo "--fade-mode must be default or legacy" >&2; exit 2 ;; esac

mkdir -p "$OUT_DIR"
if [[ "$replace_label" == "1" ]]; then
  rm -rf "$OUT_DIR/${label}-arcade.tsv" \
    "$OUT_DIR/${label}-arcade.log" \
    "$OUT_DIR/${label}-arcade.status.txt" \
    "$OUT_DIR/${label}-arcade.status.json" \
    "$OUT_DIR/${label}-arcade-chart.svg" \
    "$OUT_DIR/${label}-arcade-report.html" \
    "$OUT_DIR/${label}-arcade-cpu.svg" \
    "$OUT_DIR/${label}-visuals"
fi
env_file="$(mktemp)"

tsv_value() {
  printf '%s' "$1" | tr '\t\r\n' '   '
}

file_sha256() {
  local path="$1"
  shasum -a 256 "$path" 2>/dev/null | awk '{ print $1 }'
}

png_stats() {
  local path="$1"
  python3 - "$path" <<'PY' 2>/dev/null || printf '0\t0\tunknown\n'
import struct
import sys
import zlib

path = sys.argv[1]
with open(path, "rb") as f:
    data = f.read()
if not data.startswith(b"\x89PNG\r\n\x1a\n"):
    raise SystemExit(1)
offset = 8
width = height = None
idat = bytearray()
while offset + 8 <= len(data):
    length = struct.unpack(">I", data[offset:offset + 4])[0]
    tag = data[offset + 4:offset + 8]
    payload = data[offset + 8:offset + 8 + length]
    offset += 12 + length
    if tag == b"IHDR":
        width, height, bit_depth, color_type = struct.unpack(">IIBB", payload[:10])
        if bit_depth != 8 or color_type != 6:
            raise SystemExit(1)
    elif tag == b"IDAT":
        idat.extend(payload)
    elif tag == b"IEND":
        break
if not width or not height:
    raise SystemExit(1)
raw = zlib.decompress(bytes(idat))
stride = width * 4
first = None
nonblank = False
for y in range(height):
    row = raw[y * (stride + 1):(y + 1) * (stride + 1)]
    if not row or row[0] != 0:
        raise SystemExit(1)
    pixels = row[1:]
    for x in range(0, len(pixels), 4):
        rgb = bytes(pixels[x:x + 3])
        if first is None:
            first = rgb
        elif rgb != first:
            nonblank = True
            break
    if nonblank:
        break
print(f"{width}\t{height}\t{str(nonblank).lower()}")
PY
}

emit_artifact_row() {
  local kind="$1" local_path="$2" remote_path="${3:-}"
  local exists="false" bytes="0" sha="" width="0" height="0" nonblank="unknown"
  if [[ -f "$local_path" ]]; then
    exists="true"
    bytes="$(wc -c <"$local_path" | tr -d ' ')"
    sha="$(file_sha256 "$local_path")"
    if [[ "$local_path" == *.png ]]; then
      IFS=$'\t' read -r width height nonblank <<<"$(png_stats "$local_path")"
    fi
  fi
  printf 'artifact_tsv\tlabel=%s\tkind=%s\tlocal_path=%s\tremote_path=%s\texists=%s\tbytes=%s\tsha256=%s\twidth=%s\theight=%s\tnonblank=%s\n' \
    "$(tsv_value "$label")" "$(tsv_value "$kind")" "$(tsv_value "$local_path")" \
    "$(tsv_value "$remote_path")" "$exists" "$bytes" "$sha" "$width" "$height" "$nonblank"
}

emit_validity_row() {
  local valid="$1" reason="$2" detail="${3:-}"
  printf 'validity_tsv\tlabel=%s\tvalid=%s\tinvalid_reason=%s\tdetail=%s\n' \
    "$(tsv_value "$label")" "$valid" "$(tsv_value "$reason")" "$(tsv_value "$detail")"
}

check_composition_recovery_gate() {
  local name="$1" status_json="$2"
  python3 - "$name" "$status_json" <<'PY'
import json
import sys

name, path = sys.argv[1], sys.argv[2]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
slint = data.get("runtime", {}).get("slint_status", {})
count = int(slint.get("composition_recovery_count") or 0)
state = slint.get("composition_state") or ""
kind = slint.get("last_composition_invariant_kind") or ""
detail = slint.get("last_composition_invariant_detail") or ""
print(
    f"composition_gate_tsv\tcase={name}\tstate={state}\trecovery_count={count}\tlast_kind={kind}\tlast_detail={detail}\tvalid={1 if count == 0 else 0}"
)
raise SystemExit(0 if count == 0 else 11)
PY
}

emit_run_context_row() {
  local commit command_text started_at profile features binary_path runtime_type deployment_state binary_fields
  commit="$(git -C "$HERE" rev-parse --short HEAD 2>/dev/null || echo unknown)"
  command_text="scripts/profile-preview-scroll.sh ${ORIGINAL_ARGS[*]}"
  started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  profile="release-device"
  features="ui,bench-tools"
  runtime_type="bench-tools"
  deployment_state="unverified-skip-build"
  if [[ "$deploy" == "device" ]]; then
    deployment_state="verified"
  fi
  if [[ "$cpu_profile" == "1" ]]; then
    profile="release-device-profile"
    features="ui,profile,bench-tools"
    runtime_type="profile"
    deployment_state="verified"
  fi
  binary_path="$HERE/magik-gui/target/armv7-unknown-linux-gnueabihf/$profile/mister-magik-fb"
  binary_fields="$(bench_context_binary_fields "$profile" "launcher" "$features" "$binary_path" "$runtime_type" "$deployment_state")"
  if [[ "$thread_sample_enabled" == "1" ]]; then
    printf 'run_context_tsv\tlabel=%s\tcommit=%s\tcommand=%s\tdevice=mister\tscenario=%s\tremote_scenario=%s\tsecs=%s\tdeploy=%s\tcpu_profile=%s\tvisual_captures=%s\tskip_preview_warm=%s\tfade_mode=%s\tstarted_at=%s\t%s\tthread_sample=%s\n' \
      "$(tsv_value "$label")" "$commit" "$(tsv_value "$command_text")" \
      "$(tsv_value "$scenario")" "$(tsv_value "$remote_scenario")" "$secs" "$deploy" \
      "$cpu_profile" "$visual_captures" "$skip_preview_warm" "$fade_mode" "$started_at" "$binary_fields" "$thread_sample_enabled"
  else
    printf 'run_context_tsv\tlabel=%s\tcommit=%s\tcommand=%s\tdevice=mister\tscenario=%s\tremote_scenario=%s\tsecs=%s\tdeploy=%s\tcpu_profile=%s\tvisual_captures=%s\tskip_preview_warm=%s\tfade_mode=%s\tstarted_at=%s\t%s\n' \
      "$(tsv_value "$label")" "$commit" "$(tsv_value "$command_text")" \
      "$(tsv_value "$scenario")" "$(tsv_value "$remote_scenario")" "$secs" "$deploy" \
      "$cpu_profile" "$visual_captures" "$skip_preview_warm" "$fade_mode" "$started_at" "$binary_fields"
  fi
}

cleanup() {
  rm -f "$env_file"
  if [[ "$self_test" == "1" ]]; then return; fi
  "$MISTER" run "rm -f '$REMOTE_ENV'; if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}
trap cleanup EXIT

case "$deploy" in
  device) "$HERE/scripts/deploy-rust.sh" --device --ui-scope launcher --bench-tools ;;
  skip) : ;;
esac

if [[ "$cpu_profile" == "1" && "$self_test" != "1" ]]; then
  profile_bin="$HERE/magik-gui/target/armv7-unknown-linux-gnueabihf/release-device-profile/mister-magik-fb"
  echo "==> Build profiling binary for supervised Arcade CPU profile"
  "$HERE/magik-gui/build-arm.sh" --profile --ui-scope launcher --bench-tools
  echo "==> Deploy profiling binary for supervised Arcade CPU profile"
  if ! "$MISTER" agent deploy-magik-bin "$profile_bin" /media/fat/mister-magik/mister-magik-fb >/dev/null; then
    echo "agent deploy failed for profiling binary; falling back to device deploy transaction" >&2
    "$MISTER" deploy-magik-bin "$profile_bin" /media/fat/mister-magik/mister-magik-fb >/dev/null
  fi
fi

write_launcher_env() {
  local scenario_value="$1"
  local trace_path="$2"
  local selected_index="${3:-}"
  {
    printf 'export MISTER_CATALOG_REFRESH=default\n'
    if [[ "$defer_start_system" == "1" ]]; then
      printf 'export MISTER_LAUNCHER_START_SCREEN=home\n'
    else
      printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
      printf 'export MISTER_LAUNCHER_LOCK_SCREEN=arcade\n'
    fi
    printf 'export MISTER_LAUNCHER_START_SYSTEM=%q\n' "$start_system"
    printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=%q\n' "$scenario_value"
    printf 'export MISTER_PREVIEW_TRACE=1\n'
    printf 'export MISTER_PREVIEW_SCROLL_TRACE_SECS=%q\n' "$secs"
    if [[ -n "$trace_path" ]]; then printf 'export MISTER_PREVIEW_SCROLL_TRACE=%q\n' "$trace_path"; fi
    if [[ -n "$selected_index" ]]; then printf 'export MISTER_ARCADE_SELECTED_INDEX=%q\n' "$selected_index"; fi
    if [[ "$skip_preview_warm" == "1" || "$skip_preview_warm" == "true" || "$skip_preview_warm" == "yes" || "$skip_preview_warm" == "on" ]]; then
      printf 'export MISTER_PREVIEW_SCROLL_SKIP_ARCHIVE_WARM=1\n'
    fi
    if [[ -n "${MISTER_PREVIEW_DIRECT_PRESENT+x}" ]]; then
      printf 'export MISTER_PREVIEW_DIRECT_PRESENT=%q\n' "$MISTER_PREVIEW_DIRECT_PRESENT"
    fi
    if [[ "$fade_mode" == "legacy" ]]; then
      printf 'export MISTER_PREVIEW_FADE_P02=legacy\n'
    fi
    if [[ "$cpu_profile" == "1" ]]; then
      printf 'export MISTER_PPROF=1\n'
      printf 'export MISTER_PPROF_OUT=%q\n' "$cpu_profile_remote_svg"
      printf 'export MISTER_PREVIEW_SCROLL_EXIT_AFTER_TRACE=1\n'
    fi
  } >"$env_file"
}

restart_supervised_launcher() {
  local trace_path="$1"
  "$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
  "$MISTER" run "rm -f '$REMOTE_LOG' '$trace_path'; if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd" >/dev/null
}

run_case() {
  local name="$1"
  local remote_tsv="/tmp/${label}-${name}.tsv"
  local local_tsv="$OUT_DIR/${label}-${name}.tsv"
  local local_log="$OUT_DIR/${label}-${name}.log"
  local local_status="$OUT_DIR/${label}-${name}.status.txt"
  local local_status_json="$OUT_DIR/${label}-${name}.status.json"
  local local_chart="$OUT_DIR/${label}-${name}-chart.svg"
  local local_report="$OUT_DIR/${label}-${name}-report.html"
  local local_cpu_svg="$OUT_DIR/${label}-${name}-cpu.svg"
  cpu_profile_remote_svg="/tmp/${label}-${name}-cpu.svg"

  echo "==> $name supervised launcher Arcade scenario=$scenario remote_scenario=$remote_scenario secs=$secs start_system=$start_system selected_index=${selected_index:-default} defer_start_system=$defer_start_system transition=fixed-fade cpu_profile=$cpu_profile"
  if [[ "$cpu_profile" == "1" ]]; then
    "$MISTER" run "rm -f '$cpu_profile_remote_svg'" >/dev/null
  fi
  write_launcher_env "$remote_scenario" "$remote_tsv" "$selected_index"
  restart_supervised_launcher "$remote_tsv"
  thread_sample_start "$label" "$name" "$OUT_DIR" $((secs + 10))
  sleep $((secs + 7))
  thread_sample_finish
  if ! "$MISTER" get "$remote_tsv" "$local_tsv" >/dev/null; then
    "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null || true
    "$MISTER" status >"$local_status" 2>&1 || true
    emit_artifact_row "${name}-trace" "$local_tsv" "$remote_tsv"
    emit_artifact_row "${name}-log" "$local_log" "$REMOTE_LOG"
    emit_artifact_row "${name}-status" "$local_status" "scripts/mister status"
    emit_validity_row "0" "missing_trace" "case=$name remote_tsv=$remote_tsv local_log=$local_log status=$local_status"
    echo "$name failed; see $local_log" >&2
    exit 1
  fi
  "$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null || true
  "$MISTER" status >"$local_status" 2>&1 || true
  "$MISTER" status --json >"$local_status_json" 2>/dev/null || true
  echo "wrote $local_tsv"
  echo "wrote $local_log"
  emit_artifact_row "${name}-trace" "$local_tsv" "$remote_tsv"
  emit_artifact_row "${name}-log" "$local_log" "$REMOTE_LOG"
  emit_artifact_row "${name}-status" "$local_status" "scripts/mister status"
  emit_artifact_row "${name}-status-json" "$local_status_json" "scripts/mister status --json"
  if [[ -s "$local_status_json" ]] && ! check_composition_recovery_gate "$name" "$local_status_json"; then
    emit_validity_row "0" "composition_recovery" "case=$name status=$local_status_json"
    echo "$name composition recovery occurred; see $local_status_json" >&2
    exit 13
  fi
  "$HERE/scripts/frame-profile-chart.py" "$local_tsv" "$local_chart" --title "$label $name $scenario"
  "$HERE/scripts/frame-profile-report.py" "$local_tsv" "$local_report" --title "$label $name $scenario"
  emit_artifact_row "${name}-chart" "$local_chart" ""
  emit_artifact_row "${name}-report" "$local_report" ""
  if [[ "$cpu_profile" == "1" ]]; then
    if ! "$MISTER" get "$cpu_profile_remote_svg" "$local_cpu_svg" >/dev/null || [[ ! -s "$local_cpu_svg" ]]; then
      "$MISTER" status >"$local_status" 2>&1 || true
      emit_artifact_row "${name}-cpu-svg" "$local_cpu_svg" "$cpu_profile_remote_svg"
      emit_artifact_row "${name}-status" "$local_status" "scripts/mister status"
      emit_validity_row "0" "missing_cpu_profile" "case=$name svg=$local_cpu_svg log=$local_log"
      echo "$name CPU profile failed or produced an empty SVG; see $local_log" >&2
      exit 9
    fi
    if ! grep -q 'cpu_profile:' "$local_log"; then
      emit_artifact_row "${name}-cpu-svg" "$local_cpu_svg" "$cpu_profile_remote_svg"
      emit_validity_row "0" "missing_cpu_profile_log" "case=$name svg=$local_cpu_svg log=$local_log"
      echo "$name CPU profile log does not contain cpu_profile output; see $local_log" >&2
      exit 9
    fi
    echo "wrote $local_cpu_svg"
    emit_artifact_row "${name}-cpu-svg" "$local_cpu_svg" "$cpu_profile_remote_svg"
  fi
}

capture_visuals() {
  local count="$visual_captures"
  if [[ "$count" == "0" ]]; then return; fi
  local visual_dir="$OUT_DIR/${label}-visuals"
  mkdir -p "$visual_dir"
  local indices=(0 7 14 21 28 35 42 49)
  local i idx idx_pad png_out json_out
  for ((i = 0; i < count && i < ${#indices[@]}; i++)); do
    idx="${indices[$i]}"
    idx_pad="$(printf "%03d" "$idx")"
    png_out="$visual_dir/idx${idx_pad}.png"
    json_out="$visual_dir/idx${idx_pad}.framebuffer.json"
    echo "==> visual selected_index=$idx"
    write_launcher_env "idle" "" "$idx"
    restart_supervised_launcher "/tmp/${label}-visual-${idx_pad}.tsv"
    sleep 8
    "$MISTER" agent framebuffer-capture "$png_out" --json "$json_out" >/dev/null
    "$MISTER" get "$REMOTE_LOG" "$visual_dir/idx${idx_pad}.log" >/dev/null || true
    echo "wrote $png_out"
    emit_artifact_row "visual-idx${idx_pad}" "$png_out" "agent framebuffer_capture"
    IFS=$'\t' read -r png_w png_h png_nonblank <<<"$(png_stats "$png_out")"
    if [[ "$png_nonblank" != "true" ]]; then
      emit_validity_row "0" "blank_visual_capture" "path=$png_out width=$png_w height=$png_h"
      echo "visual capture appears blank: $png_out" >&2
      exit 10
    fi
  done
}

summarize_trace() {
  local name="$1" tsv="$2" log="$3"
  awk -v name="$name" '
    BEGIN { FS="\t" }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
    NF {
      n++
      wall = $(col["wall_us"]) + 0
      walls[n] = wall
      sum += wall
      if (wall > 16667) slow16++
      if (wall > 20000) slow20++
      state = $(col["cache_state"])
      states[state]++
    }
    END {
      if (n == 0) { printf "%s\t0\t0\t0\t0\t0\t0\t0\t0\t0\n", name; exit }
      for (i = 1; i <= n; i++) for (j = i + 1; j <= n; j++) if (walls[j] < walls[i]) { tmp = walls[i]; walls[i] = walls[j]; walls[j] = tmp }
      p95_i = int(n * 0.95); if (p95_i < 1) p95_i = 1; if (p95_i > n) p95_i = n
      printf "%s\t%d\t%.0f\t%d\t%d\t%d\t%d\t%d\t%d\t%d\n", name, n, sum / n, walls[p95_i], slow16, slow20, states["exact"] + 0, states["cached"] + 0, states["stale"] + 0, states["placeholder"] + 0
    }
  ' "$tsv" | while IFS=$'\t' read -r n frames avg p95 slow16 slow20 exact cached stale placeholder; do
    local rss
    rss="$(sed -n 's/^rss_hwm_kb=//p' "$log" | tail -1)"
    printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n" "$n" "$frames" "$avg" "$p95" "$slow16" "$slow20" "$exact" "$cached" "$stale" "$placeholder" "${rss:-0}"
  done
}

summarize_frame_pacing() {
  local name="$1" tsv="$2"
  awk -v name="$name" '
    BEGIN { FS="\t" }
    NR == 1 {
      for (i = 1; i <= NF; i++) col[$i] = i
      has_pacing = ("vsync_source" in col) && ("loop_delta_us" in col)
      next
    }
    NF && $(col["frame"]) + 0 > 30 {
      n++
      wall = $(col["wall_us"]) + 0
      work = $(col["prepare_us"]) + $(col["slint_render_us"]) + $(col["custom_draw_us"]) + $(col["fb_present_us"])
      vsync = $(col["vsync_us"]) + 0
      loop_delta = has_pacing ? ($(col["loop_delta_us"]) + 0) : 0
      source = has_pacing ? $(col["vsync_source"]) : "unknown"
      miss_streak = has_pacing ? ($(col["vsync_miss_streak"]) + 0) : 0

      walls[n] = wall
      works[n] = work
      vsyncs[n] = vsync
      loops[n] = loop_delta
      wall_sum += wall
      work_sum += work
      vsync_sum += vsync
      loop_sum += loop_delta
      if (wall > 16667) slow_wall++
      if (wall > 17000) slow_wall_17++
      if (work > 16667) work_over++
      if (vsync > 10000) high_vsync++
      if (wall > 16667 && work <= 5000 && vsync > 10000) low_work_high_vsync++
      if (wall > 16667 && work > 12000) cpu_heavy_slow++
      sources[source]++
      if (miss_streak > max_miss_streak) max_miss_streak = miss_streak
    }
    END {
      if (n == 0) {
        printf "%s\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\n", name
        exit
      }
      for (i = 1; i <= n; i++) {
        for (j = i + 1; j <= n; j++) {
          if (walls[j] < walls[i]) { tmp = walls[i]; walls[i] = walls[j]; walls[j] = tmp }
          if (works[j] < works[i]) { tmp = works[i]; works[i] = works[j]; works[j] = tmp }
          if (vsyncs[j] < vsyncs[i]) { tmp = vsyncs[i]; vsyncs[i] = vsyncs[j]; vsyncs[j] = tmp }
          if (loops[j] < loops[i]) { tmp = loops[i]; loops[i] = loops[j]; loops[j] = tmp }
        }
      }
      p95 = int(n * 0.95); if (p95 < 1) p95 = 1; if (p95 > n) p95 = n
      p99 = int(n * 0.99); if (p99 < 1) p99 = 1; if (p99 > n) p99 = n
      printf "%s\t%d\t%.0f\t%d\t%d\t%.0f\t%d\t%d\t%.0f\t%d\t%d\t%.0f\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\n",
        name, n,
        wall_sum / n, walls[p95], walls[p99],
        work_sum / n, works[p95], works[p99],
        vsync_sum / n, vsyncs[p95], vsyncs[p99],
        loop_sum / n, loops[p95], loops[p99],
        slow_wall + 0, slow_wall_17 + 0, work_over + 0, high_vsync + 0,
        low_work_high_vsync + 0, cpu_heavy_slow + 0,
        sources["vsync"] + 0, sources["fallback"] + 0,
        sources["timeout"] + 0, sources["error"] + 0,
        max_miss_streak + 0
    }
  ' "$tsv"
}

summarize_custom_draw_phases() {
  local name="$1" tsv="$2"
  awk -v name="$name" '
    BEGIN { FS="\t" }
    NR == 1 {
      for (i = 1; i <= NF; i++) col[$i] = i
      if (!("arcade_list_present_us" in col) && ("overlay_present_us" in col)) {
        col["arcade_list_present_us"] = col["overlay_present_us"]
      }
      has_phases = ("arcade_list_update_us" in col) && ("preview_blit_us" in col) && ("effect_label_us" in col)
      next
    }
    NF && has_phases && $(col["frame"]) + 0 > 30 {
      n++
      phase["custom_draw_us", n] = $(col["custom_draw_us"]) + 0
      phase["arcade_list_update_us", n] = $(col["arcade_list_update_us"]) + 0
      phase["preview_blit_us", n] = $(col["preview_blit_us"]) + 0
      phase["effect_label_us", n] = $(col["effect_label_us"]) + 0
      phase["cached_present_us", n] = $(col["cached_present_us"]) + 0
      phase["hidden_compose_us", n] = ("hidden_compose_us" in col) ? $(col["hidden_compose_us"]) + 0 : 0
      phase["hidden_preview_compose_us", n] = ("hidden_preview_compose_us" in col) ? $(col["hidden_preview_compose_us"]) + 0 : 0
      phase["hidden_arcade_compose_us", n] = ("hidden_arcade_compose_us" in col) ? $(col["hidden_arcade_compose_us"]) + 0 : 0
      phase["direct_preview_present_us", n] = ("direct_preview_present_us" in col) ? $(col["direct_preview_present_us"]) + 0 : 0
      phase["arcade_list_present_us", n] = $(col["arcade_list_present_us"]) + 0
      sum["custom_draw_us"] += phase["custom_draw_us", n]
      sum["arcade_list_update_us"] += phase["arcade_list_update_us", n]
      sum["preview_blit_us"] += phase["preview_blit_us", n]
      sum["effect_label_us"] += phase["effect_label_us", n]
      sum["cached_present_us"] += phase["cached_present_us", n]
      sum["hidden_compose_us"] += phase["hidden_compose_us", n]
      sum["hidden_preview_compose_us"] += phase["hidden_preview_compose_us", n]
      sum["hidden_arcade_compose_us"] += phase["hidden_arcade_compose_us", n]
      sum["direct_preview_present_us"] += phase["direct_preview_present_us", n]
      sum["arcade_list_present_us"] += phase["arcade_list_present_us", n]
    }
    END {
      if (!has_phases || n == 0) {
        printf "%s\tmissing\t0\t0\t0\t0\n", name
        exit
      }
      fields[1] = "custom_draw_us"
      fields[2] = "arcade_list_update_us"
      fields[3] = "preview_blit_us"
      fields[4] = "effect_label_us"
      fields[5] = "cached_present_us"
      fields[6] = "hidden_compose_us"
      fields[7] = "hidden_preview_compose_us"
      fields[8] = "hidden_arcade_compose_us"
      fields[9] = "direct_preview_present_us"
      fields[10] = "arcade_list_present_us"
      for (field_i = 1; field_i <= 10; field_i++) {
        field = fields[field_i]
        for (i = 1; i <= n; i++) sorted[i] = phase[field, i]
        for (i = 1; i <= n; i++) {
          for (j = i + 1; j <= n; j++) {
            if (sorted[j] < sorted[i]) {
              tmp = sorted[i]; sorted[i] = sorted[j]; sorted[j] = tmp
            }
          }
        }
        p95 = int(n * 0.95); if (p95 < 1) p95 = 1; if (p95 > n) p95 = n
        p99 = int(n * 0.99); if (p99 < 1) p99 = 1; if (p99 > n) p99 = n
        printf "%s\t%s\t%d\t%.0f\t%d\t%d\n", name, field, n, sum[field] / n, sorted[p95], sorted[p99]
        delete sorted
      }
    }
  ' "$tsv"
}

summarize_fade_cpu() {
  local name="$1" tsv="$2"
  awk -v name="$name" '
    BEGIN { FS="\t" }
    function col_value(field) {
      return (field in col) ? ($(col[field]) + 0) : 0
    }
    function sort_values(values, count, sorted,    i, j, tmp) {
      for (i = 1; i <= count; i++) sorted[i] = values[i]
      for (i = 1; i <= count; i++) {
        for (j = i + 1; j <= count; j++) {
          if (sorted[j] < sorted[i]) {
            tmp = sorted[i]; sorted[i] = sorted[j]; sorted[j] = tmp
          }
        }
      }
    }
    function pct(sorted, count, p,    idx) {
      if (count <= 0) return 0
      idx = int(count * p / 100)
      if (idx < 1) idx = 1
      if (idx > count) idx = count
      return sorted[idx]
    }
    NR == 1 {
      for (i = 1; i <= NF; i++) col[$i] = i
      has = ("transition_effect" in col) && ("transition_progress" in col) && ("preview_fade_cpu_us" in col) && ("preview_fade_wall_us" in col) && ("preview_fade_pixels" in col) && ("preview_fade_rows" in col) && ("preview_fade_path" in col) && ("preview_fade_alpha_bucket" in col)
      next
    }
    NF && has && $(col["frame"]) + 0 > 30 {
      progress = $(col["transition_progress"]) + 0
      if ($(col["transition_effect"]) != "fade" || progress <= 0 || progress >= 1) next
      n++
      cpu[n] = col_value("preview_fade_cpu_us")
      wall[n] = col_value("preview_fade_wall_us")
      pixels[n] = col_value("preview_fade_pixels")
      rows[n] = col_value("preview_fade_rows")
      cpu_sum += cpu[n]
      wall_sum += wall[n]
      pixel_sum += pixels[n]
      row_sum += rows[n]
      path = $(col["preview_fade_path"])
      if (path == "") path = "unknown"
      path_count[path]++
      if (!(path in path_seen)) {
        path_seen[path] = 1
        path_order[++path_order_len] = path
      }
    }
    END {
      if (!has) {
        printf "fade_cpu_tsv\tcase=%s\tvalid=0\tinvalid_reason=missing_columns\tframes=0\n", name
        exit
      }
      if (n == 0) {
        printf "fade_cpu_tsv\tcase=%s\tvalid=0\tinvalid_reason=no_active_fade_frames\tframes=0\n", name
        exit
      }
      sort_values(cpu, n, cpu_sorted)
      sort_values(wall, n, wall_sorted)
      sort_values(pixels, n, pixel_sorted)
      sort_values(rows, n, row_sorted)
      paths = ""
      for (i = 1; i <= path_order_len; i++) {
        path = path_order[i]
        paths = paths (paths == "" ? "" : ",") path ":" path_count[path]
      }
      cpu_ns_per_pixel = pixel_sum > 0 ? (cpu_sum * 1000.0 / pixel_sum) : 0
      printf "fade_cpu_tsv\tcase=%s\tvalid=1\tframes=%d\tavg_cpu_us=%.1f\tp50_cpu_us=%d\tp95_cpu_us=%d\tp99_cpu_us=%d\tmax_cpu_us=%d\tavg_wall_us=%.1f\tp95_wall_us=%d\tp99_wall_us=%d\tmax_wall_us=%d\tavg_pixels=%.0f\tp95_pixels=%d\tp99_pixels=%d\tavg_rows=%.1f\tp95_rows=%d\tp99_rows=%d\tcpu_ns_per_pixel=%.3f\tpaths=%s\n",
        name, n, cpu_sum / n, pct(cpu_sorted, n, 50), pct(cpu_sorted, n, 95),
        pct(cpu_sorted, n, 99), cpu_sorted[n], wall_sum / n,
        pct(wall_sorted, n, 95), pct(wall_sorted, n, 99), wall_sorted[n],
        pixel_sum / n, pct(pixel_sorted, n, 95), pct(pixel_sorted, n, 99),
        row_sum / n, pct(row_sorted, n, 95), pct(row_sorted, n, 99),
        cpu_ns_per_pixel, paths
    }
  ' "$tsv"
}

summarize_fade_buckets() {
  local name="$1" tsv="$2"
  awk -v name="$name" '
    BEGIN { FS="\t" }
    function col_value(field) {
      return (field in col) ? ($(col[field]) + 0) : 0
    }
    NR == 1 {
      for (i = 1; i <= NF; i++) col[$i] = i
      has = ("transition_effect" in col) && ("transition_progress" in col) && ("preview_fade_cpu_us" in col) && ("preview_fade_wall_us" in col) && ("preview_fade_pixels" in col) && ("preview_fade_path" in col) && ("preview_fade_alpha_bucket" in col)
      next
    }
    NF && has && $(col["frame"]) + 0 > 30 {
      progress = $(col["transition_progress"]) + 0
      if ($(col["transition_effect"]) != "fade" || progress <= 0 || progress >= 1) next
      bucket = col_value("preview_fade_alpha_bucket")
      path = $(col["preview_fade_path"])
      if (path == "") path = "unknown"
      key = bucket SUBSEP path
      if (!(key in seen)) {
        seen[key] = 1
        order[++order_len] = key
        bucket_by_key[key] = bucket
        path_by_key[key] = path
      }
      frames[key]++
      cpu_sum[key] += col_value("preview_fade_cpu_us")
      wall_sum[key] += col_value("preview_fade_wall_us")
      pixel_sum[key] += col_value("preview_fade_pixels")
    }
    END {
      if (!has) {
        printf "fade_bucket_tsv\tcase=%s\tvalid=0\tinvalid_reason=missing_columns\tbucket=-1\tpath=missing\tframes=0\n", name
        exit
      }
      if (order_len == 0) {
        printf "fade_bucket_tsv\tcase=%s\tvalid=0\tinvalid_reason=no_active_fade_frames\tbucket=-1\tpath=none\tframes=0\n", name
        exit
      }
      for (i = 1; i <= order_len; i++) {
        key = order[i]
        n = frames[key]
        avg_cpu = cpu_sum[key] / n
        avg_wall = wall_sum[key] / n
        avg_pixels = pixel_sum[key] / n
        ns_per_pixel = pixel_sum[key] > 0 ? (cpu_sum[key] * 1000.0 / pixel_sum[key]) : 0
        printf "fade_bucket_tsv\tcase=%s\tvalid=1\tbucket=%d\tpath=%s\tframes=%d\tavg_cpu_us=%.1f\tavg_wall_us=%.1f\tavg_pixels=%.0f\tcpu_ns_per_pixel=%.3f\n",
          name, bucket_by_key[key], path_by_key[key], n, avg_cpu, avg_wall, avg_pixels, ns_per_pixel
      }
    }
  ' "$tsv"
}

report_steady_wall_gate() {
  local name="$1" tsv="$2"
  awk -v name="$name" -v allow="$allow_hotpath_misses" '
    BEGIN { FS="\t" }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
    NF && $(col["frame"]) + 0 > 30 {
      n++
      wall = $(col["wall_us"]) + 0
      if (wall > 16667) {
        slow++
        if (slow <= 10) printf "%s steady miss frame=%s wall_us=%s\n", name, $(col["frame"]), wall > "/dev/stderr"
      }
    }
    END {
      printf "%s steady_wall_report frames_after_30=%d slow_gt_16667=%d allow=%s\n", name, n, slow + 0, allow
    }
  ' "$tsv"
}

check_steady_work_gate() {
  local name="$1" tsv="$2"
  awk -v name="$name" -v allow="$allow_hotpath_misses" '
    BEGIN { FS="\t" }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
    NF && $(col["frame"]) + 0 > 30 {
      n++
      work = $(col["prepare_us"]) + $(col["slint_render_us"]) + $(col["custom_draw_us"]) + $(col["fb_present_us"])
      if (work > 16667) {
        slow++
        if (slow <= 10) printf "%s steady work miss frame=%s work_us=%s wall_us=%s vsync_us=%s\n", name, $(col["frame"]), work, $(col["wall_us"]), $(col["vsync_us"]) > "/dev/stderr"
      }
    }
    END {
      printf "%s steady_work_gate frames_after_30=%d work_gt_16667=%d allow=%s\n", name, n, slow + 0, allow
    }
  ' "$tsv"
}

report_slow_work_attribution() {
  local name="$1" tsv="$2"
  awk -v name="$name" -v label="$label" '
    BEGIN { FS="\t" }
    function col_value(field) {
      return (field in col) ? ($(col[field]) + 0) : 0
    }
    function choose_dominant(prepare, render, custom, present,    dominant, max_us) {
      dominant = "prepare"; max_us = prepare
      if (render > max_us) { dominant = "slint_render"; max_us = render }
      if (custom > max_us) { dominant = "custom_draw"; max_us = custom }
      if (present > max_us) { dominant = "fb_present"; max_us = present }
      dominant_phase = dominant
      dominant_phase_us = max_us
    }
    NR == 1 {
      for (i = 1; i <= NF; i++) col[$i] = i
      has_base = ("frame" in col) && ("prepare_us" in col) && ("slint_render_us" in col) && ("custom_draw_us" in col) && ("fb_present_us" in col) && ("wall_us" in col)
      has_detail = ("catalog_worker_us" in col) && ("media_worker_us" in col) && ("media_gate_us" in col) && ("preview_schedule_us" in col) && ("preview_apply_us" in col) && ("preview_blit_us" in col) && ("status_string_copy_us" in col) && ("runtime_status_write_us" in col)
      next
    }
    NF && has_base && $(col["frame"]) + 0 > 30 {
      prepare = col_value("prepare_us")
      render = col_value("slint_render_us")
      custom = col_value("custom_draw_us")
      present = col_value("fb_present_us")
      work = prepare + render + custom + present
      if (work <= 16667) next

      slow++
      preview_schedule = col_value("preview_schedule_us")
      preview_apply = col_value("preview_apply_us")
      preview_blit = col_value("preview_blit_us")
      preview = preview_schedule + preview_apply + preview_blit
      catalog_worker = col_value("catalog_worker_us")
      media_worker = col_value("media_worker_us")
      worker = catalog_worker + media_worker
      media_gate = col_value("media_gate_us")
      status_copy = col_value("status_string_copy_us")
      status_write = col_value("runtime_status_write_us")
      status = status_copy + status_write
      choose_dominant(prepare, render, custom, present)

      specific_min_us = 1000
      if (!has_detail) {
        attribution = "unattributed"
        unattributed++
      } else if (preview >= specific_min_us && preview * 2 >= dominant_phase_us && preview >= worker && preview >= media_gate && preview >= status) {
        attribution = "preview"
        preview_count++
        attributed++
      } else if (worker >= specific_min_us && worker * 2 >= dominant_phase_us && worker >= media_gate && worker >= status) {
        attribution = "worker"
        worker_count++
        attributed++
      } else if (media_gate >= specific_min_us && media_gate * 2 >= dominant_phase_us && media_gate >= status) {
        attribution = "media_gate"
        media_gate_count++
        attributed++
      } else if (status >= specific_min_us && status * 2 >= dominant_phase_us) {
        attribution = "status"
        status_count++
        attributed++
      } else if (dominant_phase_us > 0) {
        attribution = "dominant_" dominant_phase
        dominant_count[dominant_phase]++
        attributed++
      } else {
        attribution = "unattributed"
        unattributed++
      }

      printf "slow_frame_attribution_tsv\tcase=%s\tframe=%d\twork_us=%d\twall_us=%d\tattribution=%s\tdominant_phase=%s\tdominant_phase_us=%d\tprepare_us=%d\tslint_render_us=%d\tcustom_draw_us=%d\tfb_present_us=%d\tpreview_us=%d\tworker_us=%d\tmedia_gate_us=%d\tstatus_us=%d\tpreview_schedule_us=%d\tpreview_apply_us=%d\tpreview_blit_us=%d\tcatalog_worker_us=%d\tmedia_worker_us=%d\tstatus_write_due=%d\tstatus_write_us=%d\n",
        name, $(col["frame"]) + 0, work, col_value("wall_us"), attribution,
        dominant_phase, dominant_phase_us, prepare, render, custom, present,
        preview, worker, media_gate, status, preview_schedule, preview_apply,
        preview_blit, catalog_worker, media_worker, col_value("status_write_due"),
        status_write
    }
    END {
      if (!has_base) {
        printf "slow_frame_attribution_summary_tsv\tcase=%s\tvalid=0\tinvalid_reason=missing_base_columns\tslow_work_frames=0\tattributed=0\tunattributed=0\n", name
        printf "metric_tsv\tlabel=%s\tcase=%s\tmetric=unattributed_slow_work_frames\tvalue=0\tunit=frames\tvalid=0\n", label, name
        exit 11
      }
      printf "slow_frame_attribution_summary_tsv\tcase=%s\tvalid=%d\tinvalid_reason=%s\tslow_work_frames=%d\tattributed=%d\tunattributed=%d\tpreview=%d\tworker=%d\tmedia_gate=%d\tstatus=%d\tdominant_prepare=%d\tdominant_slint_render=%d\tdominant_custom_draw=%d\tdominant_fb_present=%d\n",
        name, (unattributed == 0 ? 1 : 0), (unattributed == 0 ? "ok" : "unattributed_slow_work"),
        slow + 0, attributed + 0, unattributed + 0, preview_count + 0, worker_count + 0,
        media_gate_count + 0, status_count + 0, dominant_count["prepare"] + 0,
        dominant_count["slint_render"] + 0, dominant_count["custom_draw"] + 0,
        dominant_count["fb_present"] + 0
      printf "metric_tsv\tlabel=%s\tcase=%s\tmetric=unattributed_slow_work_frames\tvalue=%d\tunit=frames\tvalid=%d\n",
        label, name, unattributed + 0, (unattributed == 0 ? 1 : 0)
      if (unattributed > 0) exit 11
    }
  ' "$tsv"
}

summarize_preview_timing() {
  local name="$1" log="$2"
  awk -v name="$name" '
    /preview_trace decoded / {
      total = read = decode = decode_cpu = raw565_parse = raw565_parse_cpu = decode_plus_parse = decode_plus_parse_cpu = 0
      cache_hit = "unknown"
      load_source = "unknown"
      for (i = 1; i <= NF; i++) {
        split($i, kv, "=")
        if (kv[1] == "total_us") total = kv[2] + 0
        else if (kv[1] == "read_us") read = kv[2] + 0
        else if (kv[1] == "decode_us") decode = kv[2] + 0
        else if (kv[1] == "decode_cpu_us") decode_cpu = kv[2] + 0
        else if (kv[1] == "raw565_parse_us") raw565_parse = kv[2] + 0
        else if (kv[1] == "raw565_parse_cpu_us") raw565_parse_cpu = kv[2] + 0
        else if (kv[1] == "decode_plus_parse_us") decode_plus_parse = kv[2] + 0
        else if (kv[1] == "decode_plus_parse_cpu_us") decode_plus_parse_cpu = kv[2] + 0
        else if (kv[1] == "cache_hit") cache_hit = kv[2]
        else if (kv[1] == "load_source") load_source = kv[2]
      }
      if (decode_plus_parse == 0) decode_plus_parse = decode + raw565_parse
      if (decode_plus_parse_cpu == 0) decode_plus_parse_cpu = decode_cpu + raw565_parse_cpu
      n++
      total_sum += total; read_sum += read; decode_sum += decode
      decode_cpu_sum += decode_cpu
      raw565_parse_sum += raw565_parse; decode_plus_parse_sum += decode_plus_parse
      raw565_parse_cpu_sum += raw565_parse_cpu; decode_plus_parse_cpu_sum += decode_plus_parse_cpu
      if (total > total_max) total_max = total
      if (read > read_max) read_max = read
      if (decode > decode_max) decode_max = decode
      if (decode_cpu > decode_cpu_max) decode_cpu_max = decode_cpu
      if (raw565_parse > raw565_parse_max) raw565_parse_max = raw565_parse
      if (raw565_parse_cpu > raw565_parse_cpu_max) raw565_parse_cpu_max = raw565_parse_cpu
      if (decode_plus_parse > decode_plus_parse_max) decode_plus_parse_max = decode_plus_parse
      if (decode_plus_parse_cpu > decode_plus_parse_cpu_max) decode_plus_parse_cpu_max = decode_plus_parse_cpu
      if (cache_hit == "true" || cache_hit == "1") cache_hits++
      if (load_source == "archive_mem") archive_mem++
      else if (load_source == "decoded_cache") decoded_cache++
      else if (load_source == "index_pread") index_pread++
      else if (read > 0) unexpected_file_reads++
      if (read > 5000) slow_reads++
    }
    END {
      if (n == 0) {
        printf "%s\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\n", name
      } else {
        printf "%s\t%d\t%.0f\t%d\t%.0f\t%d\t%.0f\t%d\t%.0f\t%d\t%.0f\t%d\t%.0f\t%d\t%.0f\t%d\t%.0f\t%d\t%d\t%d\t%d\t%d\t%d\t%d\t%d\n",
          name, n, total_sum / n, total_max, read_sum / n, read_max, decode_sum / n, decode_max,
          decode_cpu_sum / n, decode_cpu_max,
          raw565_parse_sum / n, raw565_parse_max, raw565_parse_cpu_sum / n, raw565_parse_cpu_max,
          decode_plus_parse_sum / n, decode_plus_parse_max, decode_plus_parse_cpu_sum / n, decode_plus_parse_cpu_max,
          cache_hits + 0, unexpected_file_reads + 0, slow_reads + 0,
          archive_mem + 0, decoded_cache + 0, index_pread + 0,
          selected_file_reads + 0
      }
    }
  ' "$log"
}

check_preview_warm_gate() {
  local name="$1" log="$2"
  awk -v name="$name" '
    BEGIN {
      loaded = "missing"
      elapsed = 0
      skipped = 0
      failed = 0
    }
    /startup_timing[[:space:]]+preview_archive_warm[[:space:]]/ {
      for (i = 1; i <= NF; i++) {
        split($i, kv, "=")
        if (kv[1] == "loaded") loaded = kv[2]
        else if (kv[1] == "elapsed_us") elapsed = kv[2] + 0
      }
    }
    /startup_timing[[:space:]]+preview_archive_warm_skipped[[:space:]]/ { skipped = 1 }
    /startup_timing[[:space:]]+preview_archive_warm_failed[[:space:]]/ { failed = 1 }
    END {
      valid = (loaded == "1" && skipped == 0 && failed == 0) ? 1 : 0
      reason = valid ? "ok" : (failed ? "warm_failed" : (skipped ? "warm_skipped" : "warm_missing_or_not_loaded"))
      printf "warm_gate_tsv\tcase=%s\tloaded=%s\telapsed_us=%d\tvalid=%d\treason=%s\n",
        name, loaded, elapsed, valid, reason
      exit(valid ? 0 : 11)
    }
  ' "$log"
}

summarize_preview_latency() {
  local name="$1" log="$2"
  awk -v name="$name" '
    function sort_values(values, count,    i, j, tmp) {
      for (i = 1; i <= count; i++) {
        for (j = i + 1; j <= count; j++) {
          if (values[j] < values[i]) {
            tmp = values[i]; values[i] = values[j]; values[j] = tmp
          }
        }
      }
    }
    function print_metric(metric, values, count,    p95, p99) {
      if (count == 0) {
        printf "%s\t%s\t0\t0\t0\t0\n", name, metric
        return
      }
      sort_values(values, count)
      p95_raw = count * 0.95
      p99_raw = count * 0.99
      p95 = int(p95_raw); if (p95 < p95_raw) p95++
      p99 = int(p99_raw); if (p99 < p99_raw) p99++
      if (p95 < 1) p95 = 1; if (p95 > count) p95 = count
      if (p99 < 1) p99 = 1; if (p99 > count) p99 = count
      printf "%s\t%s\t%d\t%d\t%d\t%d\n", name, metric, count, values[p95], values[p99], values[count]
    }
    /preview_trace apply / {
      selected = "unknown"
      age = ""
      for (i = 1; i <= NF; i++) {
        split($i, kv, "=")
        if (kv[1] == "selected") selected = kv[2]
        else if (kv[1] == "age_us") age = kv[2] + 0
      }
      if (selected == "true" && age != "") selected_apply[++selected_apply_n] = age
    }
    /preview_trace decoded / {
      priority = "unknown"
      queue_age = ""
      for (i = 1; i <= NF; i++) {
        split($i, kv, "=")
        if (kv[1] == "priority") priority = kv[2]
        else if (kv[1] == "queue_age_us") queue_age = kv[2] + 0
      }
      if (queue_age != "") {
        if (priority == "Selected") selected_decode[++selected_decode_n] = queue_age
        else if (priority ~ /^Prefetch/) prefetch_decode[++prefetch_decode_n] = queue_age
      }
    }
    END {
      print_metric("selected_apply_age_us", selected_apply, selected_apply_n + 0)
      print_metric("selected_decode_queue_age_us", selected_decode, selected_decode_n + 0)
      print_metric("prefetch_decode_queue_age_us", prefetch_decode, prefetch_decode_n + 0)
    }
  ' "$log"
}

check_preview_hotpath_cache_gate() {
  local name="$1" log="$2"
  awk -v name="$name" -v allow="$allow_hotpath_misses" '
    /preview_trace cache_failed / {
      failed++
      selected = "unknown"
      for (i = 1; i <= NF; i++) {
        split($i, kv, "=")
        if (kv[1] == "selected") selected = kv[2]
      }
      if (selected == "true") {
        selected_failed++
        if (selected_failed <= 10) printf "%s selected preview cache_failed line=%s\n", name, $0 > "/dev/stderr"
      }
    }
    END {
      if (selected_failed > 0) {
        printf "%s preview cache coverage note: selected_cache_failed=%d cache_failed=%d\n", name, selected_failed + 0, failed + 0 > "/dev/stderr"
      }
      printf "%s preview_hotpath_cache_gate cache_failed=%d selected_cache_failed=%d allow=%s\n", name, failed + 0, selected_failed + 0, allow
    }
  ' "$log"
}

check_preview_hotpath_io_gate() {
  local name="$1" log="$2"
  awk -v name="$name" -v allow="$allow_hotpath_misses" '
    /preview_trace decoded / {
      read = 0
      load_source = "unknown"
      priority = "unknown"
      for (i = 1; i <= NF; i++) {
        split($i, kv, "=")
        if (kv[1] == "read_us") read = kv[2] + 0
        else if (kv[1] == "load_source") load_source = kv[2]
        else if (kv[1] == "priority") priority = kv[2]
      }
      if (load_source == "archive_mem") archive_backed = 1
      if (load_source == "index_pread") index_pread++
      if (load_source != "archive_mem" && load_source != "decoded_cache" && load_source != "index_pread" && read > 0) {
        unexpected_file_reads++
        if (read > 5000) slow_reads++
      }
      if (priority == "Selected" && load_source != "archive_mem" && load_source != "decoded_cache" && load_source != "index_pread" && read > 0) {
        selected_file_reads++
        if (selected_file_reads <= 10) {
          printf "%s selected preview unexpected file read: source=%s read_us=%d line=%s\n", name, load_source, read, $0 > "/dev/stderr"
        }
      }
    }
    END {
      allowed = (allow == "1" || allow == "true" || allow == "yes" || allow == "on")
      if (archive_backed && !allowed && (unexpected_file_reads > 0 || slow_reads > 0 || selected_file_reads > 0)) {
        printf "%s preview hot-path io gate failed: archive_backed=1 unexpected_file_reads=%d slow_reads=%d selected_file_reads=%d\n", name, unexpected_file_reads + 0, slow_reads + 0, selected_file_reads + 0 > "/dev/stderr"
        exit 7
      }
      printf "%s preview_hotpath_io_gate archive_backed=%d index_pread=%d unexpected_file_reads=%d slow_reads=%d selected_file_reads=%d allow=%s\n", name, archive_backed + 0, index_pread + 0, unexpected_file_reads + 0, slow_reads + 0, selected_file_reads + 0, allow
    }
  ' "$log"
}

check_preview_visibility_gate() {
  local name="$1" tsv="$2"
  awk -v name="$name" '
    BEGIN { FS="\t" }
    NR == 1 {
      for (i = 1; i <= NF; i++) col[$i] = i
      if (!("cache_state" in col)) {
        printf "%s preview visibility gate failed: missing cache_state column\n", name > "/dev/stderr"
        exit 6
      }
      next
    }
    NF {
      n++
      state = $(col["cache_state"])
      states[state]++
      if (state != "exact") {
        non_exact++
        if (non_exact <= 10) {
          frame = ("frame" in col) ? $(col["frame"]) : n
          selected = ("selected" in col) ? $(col["selected"]) : "?"
          printf "%s preview visibility miss frame=%s selected=%s cache_state=%s\n", name, frame, selected, state > "/dev/stderr"
        }
      }
    }
    END {
      exact = states["exact"] + 0
      if (n == 0) {
        printf "%s preview visibility gate failed: frames=0\n", name > "/dev/stderr"
        exit 6
      }
      if (non_exact > 0) {
        printf "%s preview visibility gate failed: frames=%d exact=%d non_exact=%d\n", name, n, exact, non_exact > "/dev/stderr"
        exit 6
      }
      printf "%s preview_visibility_gate frames=%d exact=%d non_exact=0\n", name, n, exact
    }
  ' "$tsv"
}

summarize_trace_by_effect() {
  local tsv="$1"
  awk '
    BEGIN { FS="\t" }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; has_effect = ("transition_effect" in col); next }
    NF && has_effect {
      effect = $(col["transition_effect"])
      idx = ++n[effect]
      wall = $(col["wall_us"]) + 0
      walls[effect, idx] = wall
      sum[effect] += wall
      if (wall > 16667) slow16[effect]++
      if (wall > 20000) slow20[effect]++
      if (!(effect in seen)) { seen[effect] = 1; order[++order_n] = effect }
    }
    END {
      if (!has_effect) exit
      for (oi = 1; oi <= order_n; oi++) {
        effect = order[oi]; count = n[effect]
        for (i = 1; i <= count; i++) sorted[i] = walls[effect, i]
        for (i = 1; i <= count; i++) for (j = i + 1; j <= count; j++) if (sorted[j] < sorted[i]) { tmp = sorted[i]; sorted[i] = sorted[j]; sorted[j] = tmp }
        p95_i = int(count * 0.95); if (p95_i < 1) p95_i = 1; if (p95_i > count) p95_i = count
        printf "%s\t%d\t%.0f\t%d\t%d\t%d\n", effect, count, sum[effect] / count, sorted[p95_i], slow16[effect] + 0, slow20[effect] + 0
        delete sorted
      }
    }
  ' "$tsv"
}

check_velocity_motion() {
  local name="$1" tsv="$2" scenario_name="${3:-}"
  awk -v name="$name" -v scenario="$scenario_name" '
    BEGIN { FS="\t" }
    NR == 1 { for (i = 1; i <= NF; i++) col[$i] = i; next }
    NF {
      n++
      vi = $(col["visual_index"]) + 0
      selected = ("selected" in col) ? ($(col["selected"]) + 0) : 0
      if (n == 1) {
        min_vi = vi
        max_vi = vi
        min_selected = selected
        max_selected = selected
      }
      if (vi < min_vi) min_vi = vi
      if (vi > max_vi) max_vi = vi
      if (selected < min_selected) min_selected = selected
      if (selected > max_selected) max_selected = selected
      frac = vi - int(vi); if (frac < 0) frac = -frac
      if (frac > 0.001 && frac < 0.999) fractional++
      if (seen) { delta = vi - last; if (delta < 0) delta = -delta; if (delta > 0.001) moving++ }
      last = vi; seen = 1
    }
    END {
      valid = 1
      reason = "ok"
      requires_motion = (scenario == "held-scroll" || scenario == "velocity-scroll" || scenario == "turbo-hold")
      if (requires_motion && n == 0) {
        valid = 0
        reason = "no_frames"
      } else if (requires_motion && moving == 0) {
        valid = 0
        reason = "no_motion"
      } else if (moving > 0 && fractional == 0) {
        valid = 0
        reason = "no_fractional_motion"
      }
      printf "%s\t%d\t%d\t%d\n", name, n, fractional, moving
      printf "motion_valid_tsv\tcase=%s\tscenario=%s\tvalid=%d\tinvalid_reason=%s\tframes=%d\tfractional_visual_index_frames=%d\tmoving_frames=%d\tvisual_min=%.3f\tvisual_max=%.3f\tselected_min=%d\tselected_max=%d\n",
        name, scenario, valid, reason, n, fractional + 0, moving + 0, min_vi, max_vi, min_selected, max_selected
      if (requires_motion && n == 0) {
        printf "%s motion gate failed: scenario=%s reason=no_frames\n", name, scenario > "/dev/stderr"
        exit 8
      }
      if (requires_motion && moving == 0) {
        printf "%s motion gate failed: scenario=%s reason=no_motion frames=%d visual_min=%.3f visual_max=%.3f selected_min=%d selected_max=%d\n",
          name, scenario, n, min_vi, max_vi, min_selected, max_selected > "/dev/stderr"
        exit 8
      }
      if (moving > 0 && fractional == 0) {
        printf "%s motion gate failed: scenario=%s reason=no_fractional_motion frames=%d moving=%d\n",
          name, scenario, n, moving > "/dev/stderr"
        exit 3
      }
    }
  ' "$tsv"
}

run_self_test() {
  local tmp
  tmp="$(mktemp -d)"

  local no_exact="$tmp/no-exact.tsv"
  local exact="$tmp/exact.tsv"
  local mixed="$tmp/mixed.tsv"
  cat >"$no_exact" <<'EOF'
frame	cache_state
0	cached
1	placeholder
EOF
  cat >"$mixed" <<'EOF'
frame	selected	cache_state
0	0	exact
1	1	stale
EOF

  if check_preview_visibility_gate selftest "$no_exact" >/dev/null 2>&1; then
    echo "preview visibility self-test expected all-non-exact failure" >&2
    rm -rf "$tmp"
    exit 1
  fi
  if check_preview_visibility_gate selftest "$mixed" >/dev/null 2>&1; then
    echo "preview visibility self-test expected mixed exact/stale failure" >&2
    rm -rf "$tmp"
    exit 1
  fi
  cat >"$exact" <<'EOF'
frame	cache_state
0	exact
1	exact
EOF
  check_preview_visibility_gate selftest "$exact" >/dev/null

  local wall_wait_ok="$tmp/wall-wait-ok.tsv"
  local work_slow="$tmp/work-slow.tsv"
  cat >"$wall_wait_ok" <<'EOF'
frame	prepare_us	slint_render_us	custom_draw_us	vsync_us	fb_present_us	wall_us
31	100	100	100	16000	500	16800
EOF
  cat >"$work_slow" <<'EOF'
frame	prepare_us	slint_render_us	custom_draw_us	vsync_us	fb_present_us	wall_us
31	1000	1000	14000	500	1000	17500
EOF
  check_steady_work_gate selftest "$wall_wait_ok" >/dev/null
  check_steady_work_gate selftest "$work_slow" >/dev/null
  if report_slow_work_attribution selftest "$work_slow" >/dev/null 2>&1; then
    echo "slow-frame attribution self-test expected missing detail failure" >&2
    rm -rf "$tmp"
    exit 1
  fi

  local work_slow_attributed="$tmp/work-slow-attributed.tsv"
  cat >"$work_slow_attributed" <<'EOF'
frame	prepare_us	catalog_worker_us	media_worker_us	media_gate_us	preview_schedule_us	preview_apply_us	slint_render_us	custom_draw_us	arcade_list_update_us	preview_blit_us	effect_label_us	vsync_us	fb_present_us	cached_present_us	hidden_compose_us	hidden_preview_compose_us	hidden_arcade_compose_us	direct_preview_present_us	arcade_list_present_us	status_write_due	status_string_copy_us	status_string_copy_bytes	runtime_status_write_us	wall_us
31	1000	0	0	0	0	900	1000	14000	0	3000	0	500	1000	500	0	0	0	0	500	1	12	4	200	17500
EOF
  report_slow_work_attribution selftest "$work_slow_attributed" >/dev/null

  local unexpected_read="$tmp/unexpected-read.log"
  local archive_mem_ok="$tmp/archive-mem-ok.log"
cat >"$unexpected_read" <<'EOF'
preview_trace decoded generation=1 priority=Selected queue_age_us=7 cache_hit=0 load_source=unknown format=raw-rgb565 filter=hybrid source=320x224 output=320x224 total_us=1000 read_us=900 decode_us=100 resize_us=0 encoded_bytes=100 decoded_bytes=100 path=a.png
preview_trace decoded generation=2 priority=Prefetch cache_hit=0 load_source=archive_mem format=raw-rgb565 filter=hybrid source=320x224 output=320x224 total_us=300 read_us=0 decode_us=300 resize_us=0 encoded_bytes=100 decoded_bytes=100 path=b.png
EOF
  cat >"$archive_mem_ok" <<'EOF'
preview_trace decoded generation=1 priority=Selected queue_age_us=7 cache_hit=0 load_source=archive_mem format=raw-rgb565 filter=hybrid source=320x224 output=320x224 total_us=300 read_us=0 decode_us=300 resize_us=0 encoded_bytes=100 decoded_bytes=100 path=a.png
preview_trace apply generation=1 priority=Selected selected=true age_us=1 load_source=archive_mem format=raw-rgb565 filter=Hybrid source=320x224 output=320x224 total_us=300 read_us=0 decode_us=300 resize_us=0 encoded_bytes=100 decoded_bytes=100 path=a.png
preview_trace decoded generation=2 priority=Prefetch queue_age_us=11 cache_hit=0 load_source=archive_mem format=raw-rgb565 filter=hybrid source=320x224 output=320x224 total_us=300 read_us=0 decode_us=300 resize_us=0 encoded_bytes=100 decoded_bytes=100 path=b.png
EOF
  if check_preview_hotpath_io_gate selftest "$unexpected_read" >/dev/null 2>&1; then
    echo "preview hot-path io self-test expected unexpected file read failure" >&2
    rm -rf "$tmp"
    exit 1
  fi
  check_preview_hotpath_io_gate selftest "$archive_mem_ok" >/dev/null

  local no_motion="$tmp/no-motion.tsv"
  cat >"$no_motion" <<'EOF'
frame	selected	visual_index
0	0	0
1	0	0
2	0	0
EOF
  if check_velocity_motion selftest "$no_motion" held-scroll >/dev/null 2>&1; then
    echo "preview motion self-test expected held-scroll zero-motion failure" >&2
    rm -rf "$tmp"
    exit 1
  fi

  rm -rf "$tmp"
  echo "profile-preview-scroll self-test ok"
}

if [[ "$self_test" == "1" ]]; then
  run_self_test
  exit 0
fi

emit_run_context_row
run_case arcade
capture_visuals

arcade_tsv="$OUT_DIR/${label}-arcade.tsv"
arcade_log="$OUT_DIR/${label}-arcade.log"

echo
echo $'case\tframes\tavg_wall_us\tp95_wall_us\tslow_gt_16_7ms\tslow_gt_20ms\texact\tcached\tstale\tplaceholder\trss_hwm_kb'
summarize_trace arcade "$arcade_tsv" "$arcade_log"

echo
echo $'transition_effect\tframes\tavg_wall_us\tp95_wall_us\tslow_gt_16_7ms\tslow_gt_20ms'
summarize_trace_by_effect "$arcade_tsv"

echo
echo $'motion_check\tframes\tfractional_visual_index_frames\tmoving_frames'
if ! check_velocity_motion arcade "$arcade_tsv" "$remote_scenario"; then
  emit_artifact_row "arcade-trace" "$arcade_tsv" "/tmp/${label}-arcade.tsv"
  emit_artifact_row "arcade-log" "$arcade_log" "$REMOTE_LOG"
  emit_validity_row "0" "motion_gate" "scenario=$remote_scenario trace=$arcade_tsv log=$arcade_log"
  exit 8
fi

echo
echo "preview trace counts:"
printf "arcade decoded=%s apply=%s\n" \
  "$(grep -c 'preview_trace decoded' "$arcade_log" 2>/dev/null || true)" \
  "$(grep -c 'preview_trace apply' "$arcade_log" 2>/dev/null || true)"

echo
if ! check_preview_warm_gate arcade "$arcade_log"; then
  if [[ "$remote_scenario" == "turbo-hold" && "$skip_preview_warm" != "1" && "$skip_preview_warm" != "true" && "$skip_preview_warm" != "yes" && "$skip_preview_warm" != "on" ]]; then
    emit_artifact_row "arcade-trace" "$arcade_tsv" "/tmp/${label}-arcade.tsv"
    emit_artifact_row "arcade-log" "$arcade_log" "$REMOTE_LOG"
    emit_validity_row "0" "preview_warm_gate" "scenario=$remote_scenario log=$arcade_log"
    exit 12
  fi
fi

echo
echo $'preview_timing\trows\tavg_total_us\tmax_total_us\tavg_read_us\tmax_read_us\tavg_decode_us\tmax_decode_us\tavg_decode_cpu_us\tmax_decode_cpu_us\tavg_raw565_parse_us\tmax_raw565_parse_us\tavg_raw565_parse_cpu_us\tmax_raw565_parse_cpu_us\tavg_decode_plus_parse_us\tmax_decode_plus_parse_us\tavg_decode_plus_parse_cpu_us\tmax_decode_plus_parse_cpu_us\tcache_hits\tunexpected_file_reads\tslow_reads\tarchive_mem\tdecoded_cache\tindex_pread\tselected_file_reads'
summarize_preview_timing arcade "$arcade_log"

echo
echo $'preview_latency\tmetric\trows\tp95_us\tp99_us\tmax_us'
summarize_preview_latency arcade "$arcade_log"

echo
echo $'frame_pacing\tframes_after_30\tavg_wall_us\tp95_wall_us\tp99_wall_us\tavg_work_us\tp95_work_us\tp99_work_us\tavg_vsync_us\tp95_vsync_us\tp99_vsync_us\tavg_loop_delta_us\tp95_loop_delta_us\tp99_loop_delta_us\tslow_wall_gt_16_7ms\tslow_wall_gt_17ms\twork_gt_16_7ms\tvsync_gt_10ms\tlow_work_high_vsync_slow\tcpu_heavy_slow\tvsync_source_vsync\tvsync_source_fallback\tvsync_source_timeout\tvsync_source_error\tmax_vsync_miss_streak'
summarize_frame_pacing arcade "$arcade_tsv"

echo
echo $'custom_draw_phase\tphase\tframes_after_30\tavg_us\tp95_us\tp99_us'
summarize_custom_draw_phases arcade "$arcade_tsv"

echo
echo $'fade_cpu_tsv\tcase\tvalid\tframes\tavg_cpu_us\tp50_cpu_us\tp95_cpu_us\tp99_cpu_us\tmax_cpu_us\tavg_wall_us\tp95_wall_us\tp99_wall_us\tmax_wall_us\tavg_pixels\tp95_pixels\tp99_pixels\tavg_rows\tp95_rows\tp99_rows\tcpu_ns_per_pixel\tpaths'
summarize_fade_cpu arcade "$arcade_tsv"

echo
echo $'fade_bucket_tsv\tcase\tvalid\tbucket\tpath\tframes\tavg_cpu_us\tavg_wall_us\tavg_pixels\tcpu_ns_per_pixel'
summarize_fade_buckets arcade "$arcade_tsv"

echo
if ! "$PRESENT_TRACE" summarize "$arcade_tsv" --case arcade; then
  emit_artifact_row "arcade-trace" "$arcade_tsv" "/tmp/${label}-arcade.tsv"
  emit_artifact_row "arcade-log" "$arcade_log" "$REMOTE_LOG"
  emit_validity_row "0" "present_path_summary" "trace=$arcade_tsv log=$arcade_log"
  exit 12
fi

echo
if ! report_slow_work_attribution arcade "$arcade_tsv"; then
  emit_artifact_row "arcade-trace" "$arcade_tsv" "/tmp/${label}-arcade.tsv"
  emit_artifact_row "arcade-log" "$arcade_log" "$REMOTE_LOG"
  emit_validity_row "0" "slow_frame_unattributed" "trace=$arcade_tsv log=$arcade_log"
  exit 11
fi

echo
check_preview_hotpath_cache_gate arcade "$arcade_log"
check_preview_hotpath_io_gate arcade "$arcade_log"
check_preview_visibility_gate arcade "$arcade_tsv"
check_steady_work_gate arcade "$arcade_tsv"
report_steady_wall_gate arcade "$arcade_tsv"
emit_validity_row "1" "ok" "trace=$arcade_tsv log=$arcade_log"
