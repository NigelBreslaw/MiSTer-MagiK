#!/usr/bin/env bash
# Benchmark a toolchain configuration: host compile time + binary size, then the
# production launcher or media scene on the MiSTer (timings, CPU, framebuffer PNG).
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-toolchain.sh A0 --clean
#
# Appends one row per scene to history/toolchain-bench/results.tsv.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$HERE/magik-gui"
source "$HERE/scripts/mister-supervision-lib.sh"
BUILD_PROFILE=release-device
BUILD_FLAG=(--device)
REMOTE="/media/fat/mister-magik/mister-magik-fb"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results.tsv"
MISTER="$HERE/scripts/mister"

# Active benchmark scene set; retired synthetic Slint scenes are archived under
# history/bench-scenes/.
BENCH_SCENES=(launcher)
VIDEO_SRC_DIR="${MISTER_VIDEO_SRC_DIR:-$HERE/build/video-snaps-neogeo-halfres}"
VIDEO_REMOTE_DIR="${MISTER_VIDEO_REMOTE_DIR:-/media/fat/mister-magik/video-snaps/neogeo}"

export MISTER_IP="${MISTER_IP:-192.168.1.117}"
export MISTER_PASS="${MISTER_PASS:-1}"
export RUSTC_WRAPPER=""

LABEL="A0"
DO_CLEAN=0
SKIP_BUILD=0
SKIP_DEVICE=0
REPLACE_LABEL=0
INCLUDE_VIDEO=0
VIDEO_LAB=0
SCENE_FILTER=0
SELF_TEST=0
SCENE_SECS=15
SETTLE_SECS="${MISTER_BENCH_SETTLE_SECS:-5}"
FRAME_ORDER="${MISTER_FRAME_ORDER:-render-then-vsync}"
DIRTY_RECT_BROAD_PCT="${MISTER_DIRTY_RECT_BROAD_PCT:-85}"
LAUNCHER_SCENARIO="${MISTER_LAUNCHER_BENCH_SCENARIO:-}"
LAUNCHER_DIRTY_OPT="${MISTER_LAUNCHER_DIRTY_OPT:-on}"
VIDEO_RENDER_MODE="${MISTER_VIDEO_RENDER_MODE:-direct-blit}"
VIDEO_QUEUE_DEPTH="${MISTER_VIDEO_QUEUE_DEPTH:-2}"
VIDEO_SCALE="${MISTER_VIDEO_SCALE:-source}"
VIDEO_PROFILE="${MISTER_VIDEO_PROFILE:-summary}"
VIDEO_THREADS="${MISTER_VIDEO_THREADS:-}"
VIDEO_THREAD_TYPE="${MISTER_VIDEO_THREAD_TYPE:-none}"
VIDEO_CONVERT="${MISTER_VIDEO_CONVERT:-custom-neon}"
UI_SCOPE="${MISTER_UI_BUILD_SCOPE:-launcher}"
MIN_FPS="${MISTER_BENCH_MIN_FPS:-55}"
MAX_VSYNC_FALLBACK="${MISTER_BENCH_MAX_VSYNC_FALLBACK:-0}"
MAX_VSYNC_ERRORS="${MISTER_BENCH_MAX_VSYNC_ERRORS:-0}"
MAX_RENDER_US="${MISTER_BENCH_MAX_RENDER_US:-}"
MAX_COPY_US="${MISTER_BENCH_MAX_COPY_US:-}"
BENCH_FAILURES=0

usage() {
  sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'
  echo ""
  echo "Scenes: ${BENCH_SCENES[*]}"
  echo ""
  echo "Options: --clean  --skip-build  --skip-device  --replace-label  --scene-secs N"
  echo "         --device (default; build profile release-device / A3)  --video  --video-lab  --scene NAME  --self-test  -h"
  echo "         --frame-order render-then-vsync|vsync-first"
  echo "         --dirty-rect-broad-pct N"
  echo "         --launcher-scenario idle|home-nav|home-repeat-hold|velocity-scroll|quick-tap|rapid-taps|held-scroll|turbo-hold|preview-step-hold|model-sync"
  echo "         --launcher-dirty-opt on|off"
  echo "         --video-render-mode direct-blit (slint-image requires --video-lab)"
  echo "         --video-queue-depth N  --video-scale source (fit-height|fit-width|native require --video-lab)"
  echo "         --video-profile summary|full|trace  --video-threads N  --video-thread-type none|frame|slice|auto (--video-threads/thread modes require --video-lab)"
  echo "         --video-convert custom-neon (swscale-rgb565 requires --video-lab)"
  echo "         --ui-scope launcher|arcade|all (default: launcher)"
  echo "  (--ui-secs N is an alias for --scene-secs)"
  echo ""
  echo "Gate env: MISTER_BENCH_MIN_FPS=$MIN_FPS, MISTER_BENCH_MAX_VSYNC_FALLBACK=$MAX_VSYNC_FALLBACK,"
  echo "          MISTER_BENCH_MAX_VSYNC_ERRORS=$MAX_VSYNC_ERRORS,"
  echo "          optional MISTER_BENCH_MAX_RENDER_US / MISTER_BENCH_MAX_COPY_US"
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage 0 ;;
    --clean) DO_CLEAN=1; shift ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --skip-device) SKIP_DEVICE=1; shift ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    --self-test) SELF_TEST=1; shift ;;
    --device) BUILD_PROFILE=release-device; shift ;;
    --video) INCLUDE_VIDEO=1; BUILD_FLAG+=(--video); shift ;;
    --video-lab) INCLUDE_VIDEO=1; VIDEO_LAB=1; BUILD_FLAG+=(--video-lab); shift ;;
    --scene)
      SCENE_FILTER=1
      BENCH_SCENES=("${2:?}")
      if [[ "$2" == "video_playback" ]]; then
        INCLUDE_VIDEO=1
        BUILD_FLAG+=(--video)
      fi
      shift 2
      ;;
    --scene-secs|--ui-secs) SCENE_SECS="${2:?}"; shift 2 ;;
    --frame-order) FRAME_ORDER="${2:?}"; shift 2 ;;
    --dirty-rect-broad-pct) DIRTY_RECT_BROAD_PCT="${2:?}"; shift 2 ;;
    --launcher-scenario) LAUNCHER_SCENARIO="${2:?}"; shift 2 ;;
    --launcher-dirty-opt) LAUNCHER_DIRTY_OPT="${2:?}"; shift 2 ;;
    --video-render-mode) VIDEO_RENDER_MODE="${2:?}"; shift 2 ;;
    --video-queue-depth) VIDEO_QUEUE_DEPTH="${2:?}"; shift 2 ;;
    --video-scale) VIDEO_SCALE="${2:?}"; shift 2 ;;
    --video-profile) VIDEO_PROFILE="${2:?}"; shift 2 ;;
    --video-threads) VIDEO_THREADS="${2:?}"; shift 2 ;;
    --video-thread-type) VIDEO_THREAD_TYPE="${2:?}"; shift 2 ;;
    --video-convert) VIDEO_CONVERT="${2:?}"; shift 2 ;;
    --ui-scope) UI_SCOPE="${2:?}"; shift 2 ;;
    -*) echo "Unknown option: $1" >&2; usage 1 ;;
    *) LABEL="$1"; shift ;;
  esac
done

case "$FRAME_ORDER" in
  render-then-vsync|vsync-first) ;;
  *) echo "Unknown --frame-order: $FRAME_ORDER (use render-then-vsync|vsync-first)" >&2; exit 1 ;;
esac
case "$DIRTY_RECT_BROAD_PCT" in
  ''|*[!0-9]*) echo "Invalid --dirty-rect-broad-pct: $DIRTY_RECT_BROAD_PCT" >&2; exit 1 ;;
  *) ;;
esac
if [[ "$DIRTY_RECT_BROAD_PCT" -lt 1 || "$DIRTY_RECT_BROAD_PCT" -gt 100 ]]; then
  echo "Invalid --dirty-rect-broad-pct: $DIRTY_RECT_BROAD_PCT (use 1..100)" >&2
  exit 1
fi
if [[ -n "$LAUNCHER_SCENARIO" ]]; then
  case "$LAUNCHER_SCENARIO" in
    idle|home-nav|home-repeat-hold|velocity-scroll|quick-tap|rapid-taps|held-scroll|turbo-hold|preview-step-hold|model-sync) ;;
    *) echo "Unknown --launcher-scenario: $LAUNCHER_SCENARIO" >&2; exit 1 ;;
  esac
  BUILD_FLAG+=(--bench-tools)
fi
case "$LAUNCHER_DIRTY_OPT" in
  on|off|1|0|true|false) ;;
  *) echo "Unknown --launcher-dirty-opt: $LAUNCHER_DIRTY_OPT" >&2; exit 1 ;;
esac
case "$VIDEO_RENDER_MODE" in
  slint-image|direct-blit) ;;
  *) echo "Unknown --video-render-mode: $VIDEO_RENDER_MODE" >&2; exit 1 ;;
esac
case "$VIDEO_QUEUE_DEPTH" in
  ''|*[!0-9]*) echo "Invalid --video-queue-depth: $VIDEO_QUEUE_DEPTH" >&2; exit 1 ;;
  *) ;;
esac
case "$VIDEO_SCALE" in
  source|fit-height|fit-width|native) ;;
  *) echo "Unknown --video-scale: $VIDEO_SCALE" >&2; exit 1 ;;
esac
case "$VIDEO_PROFILE" in
  summary|full|trace) ;;
  *) echo "Unknown --video-profile: $VIDEO_PROFILE" >&2; exit 1 ;;
esac
case "$VIDEO_THREADS" in
  '') ;;
  *[!0-9]*) echo "Invalid --video-threads: $VIDEO_THREADS" >&2; exit 1 ;;
  *) ;;
esac
if [[ "$VIDEO_RENDER_MODE" != "direct-blit" || "$VIDEO_SCALE" != "source" || -n "$VIDEO_THREADS" || "$VIDEO_THREAD_TYPE" != "none" || "$VIDEO_CONVERT" != "custom-neon" ]]; then
  if [[ "$VIDEO_LAB" -ne 1 ]]; then
    echo "Video comparison/fallback options require --video-lab; production --video supports direct-blit/source/custom-neon/thread-type none only." >&2
    exit 1
  fi
fi
case "$UI_SCOPE" in
  all|launcher|arcade) ;;
  *) echo "Unknown --ui-scope: $UI_SCOPE (use all|launcher|arcade)" >&2; exit 1 ;;
esac
if [[ "$SCENE_FILTER" -eq 0 && -n "$LAUNCHER_SCENARIO" ]]; then
  BENCH_SCENES=(launcher)
elif [[ "$SCENE_FILTER" -eq 0 && "$UI_SCOPE" == "arcade" ]]; then
  BENCH_SCENES=(launcher)
elif [[ "$SCENE_FILTER" -eq 0 && "$UI_SCOPE" == "launcher" ]]; then
  BENCH_SCENES=(launcher)
fi
if [[ "$UI_SCOPE" == "all" ]]; then
  BUILD_FLAG+=(--all-scenes)
fi
for numeric_var in MIN_FPS MAX_VSYNC_FALLBACK MAX_VSYNC_ERRORS; do
  numeric_value="${!numeric_var}"
  case "$numeric_value" in
    ''|*[!0-9]*) echo "Invalid $numeric_var: $numeric_value" >&2; exit 1 ;;
    *) ;;
  esac
done
for optional_numeric_var in MAX_RENDER_US MAX_COPY_US; do
  optional_numeric_value="${!optional_numeric_var}"
  case "$optional_numeric_value" in
    '') ;;
    *[!0-9]*) echo "Invalid $optional_numeric_var: $optional_numeric_value" >&2; exit 1 ;;
    *) ;;
  esac
done

if [[ "$INCLUDE_VIDEO" -eq 1 && "$SCENE_FILTER" -eq 0 ]]; then
  BENCH_SCENES+=(video_playback)
fi

BIN="$RUST_DIR/target/armv7-unknown-linux-gnueabihf/$BUILD_PROFILE/mister-magik-fb"

mkdir -p "$BENCH_DIR"

OLD_TSV_HEADER="label	scene	date	rustc	compile_sec	bytes	render_us	vsync_us	copy_us	rows_avg	fps	cpu_mean	cpu_max	rss_kb	visual_ok	notes"
SPLIT_TSV_HEADER="label	scene	date	rustc	compile_sec	bytes	render_us	vsync_us	copy_us	rows_avg	fps	cpu_mean	cpu_max	rss_kb	visual_ok	timing_ok	capture_ok	notes"
OVERLAY_PRESENT_TSV_HEADER="label	scene	date	rustc	compile_sec	bytes	prepare_us	render_us	custom_draw_us	vsync_us	copy_us	cached_present_us	overlay_present_us	rows_avg	fps	cpu_mean	cpu_max	rss_kb	visual_ok	timing_ok	capture_ok	notes"
TSV_HEADER="label	scene	date	rustc	compile_sec	bytes	prepare_us	render_us	custom_draw_us	vsync_us	copy_us	cached_present_us	arcade_list_present_us	rows_avg	fps	cpu_mean	cpu_max	rss_kb	visual_ok	timing_ok	capture_ok	notes"

normalize_results_tsv() {
  local tsv="$1"
  local header tmp_tsv
  if [[ ! -f "$tsv" ]] || ! head -1 "$tsv" | grep -q $'^label\tscene'; then
    echo "$TSV_HEADER" >"$tsv"
    return 0
  fi
  header="$(head -1 "$tsv")"
  if [[ "$header" == "$TSV_HEADER" ]]; then
    return 0
  fi
  if [[ "$header" == "$OLD_TSV_HEADER" ]]; then
    tmp_tsv="$(mktemp)"
    awk -v new_header="$TSV_HEADER" 'BEGIN { FS = OFS = "\t" }
      NR == 1 { print new_header; next }
      NF == 0 { next }
      {
        notes = $16
        if (notes == "") notes = "legacy-no-notes"
        print $1, $2, $3, $4, $5, $6, "", $7, "", $8, $9, "", "", $10, $11, $12, $13, $14, $15, "", "", notes
      }
    ' "$tsv" >"$tmp_tsv"
    mv "$tmp_tsv" "$tsv"
    return 0
  fi
  if [[ "$header" == "$SPLIT_TSV_HEADER" ]]; then
    tmp_tsv="$(mktemp)"
    awk -v new_header="$TSV_HEADER" 'BEGIN { FS = OFS = "\t" }
      NR == 1 { print new_header; next }
      NF == 0 { next }
      {
        notes = $18
        if (notes == "") notes = "legacy-no-notes"
        print $1, $2, $3, $4, $5, $6, "", $7, "", $8, $9, "", "", $10, $11, $12, $13, $14, $15, $16, $17, notes
      }
    ' "$tsv" >"$tmp_tsv"
    mv "$tmp_tsv" "$tsv"
    return 0
  fi
  if [[ "$header" == "$OVERLAY_PRESENT_TSV_HEADER" ]]; then
    tmp_tsv="$(mktemp)"
    awk -v new_header="$TSV_HEADER" 'BEGIN { FS = OFS = "\t" }
      NR == 1 { print new_header; next }
      NF == 0 { next }
      { print }
    ' "$tsv" >"$tmp_tsv"
    mv "$tmp_tsv" "$tsv"
    return 0
  fi
  echo "Unsupported $tsv header: $header" >&2
  return 1
}

mister() {
  "$MISTER" "$@"
}

rustc_version() {
  (cd "$RUST_DIR" && rustc -V 2>/dev/null | awk '{print $2}')
}

parse_ui_log() {
  local ui_log="$1"
  awk '
function number_after(line, key, rest) {
  rest = line
  if (index(rest, key) == 0) return ""
  rest = substr(rest, index(rest, key) + length(key))
  sub(/^[^0-9]*/, "", rest)
  sub(/[^0-9].*/, "", rest)
  return rest + 0
}
function rows_avg(line, rest) {
  rest = line
  if (index(rest, "(") == 0 || index(rest, "rows avg") == 0) return 0
  sub(/^.*\(/, "", rest)
  sub(/ .*/, "", rest)
  return rest + 0
}
/fps ~ / {
  fps = number_after($0, "fps ~")
  if (index($0, "ram-scroll ") > 0) {
    render = number_after($0, "ram-scroll ")
    vsync = number_after($0, "exposed-strip ")
    copy = number_after($0, "fb-copy ")
    rows = 0
  } else if ((index($0, "slint-render ") > 0 || index($0, "render ") > 0) && index($0, "vsync-wait ") > 0 && (index($0, "fb-present ") > 0 || index($0, "copy ") > 0)) {
    prepare = number_after($0, "prepare ")
    if (index($0, "slint-render ") > 0) render = number_after($0, "slint-render ")
    else render = number_after($0, "render ")
    custom_draw = number_after($0, "custom-draw ")
    vsync = number_after($0, "vsync-wait ")
    if (index($0, "fb-present ") > 0) copy = number_after($0, "fb-present ")
    else copy = number_after($0, "copy ")
    cached_present = number_after($0, "cached-present ")
    overlay_present = number_after($0, "arcade-list-present ")
    rows = rows_avg($0)
  } else {
    next
  }
  count++
  if (count <= 3) next
  n++
  prepare_sum += prepare
  render_sum += render
  custom_draw_sum += custom_draw
  vsync_sum += vsync
  copy_sum += copy
  cached_present_sum += cached_present
  overlay_present_sum += overlay_present
  rows_sum += rows
  fps_sum += fps
}
END {
  if (n > 0) {
    print int(render_sum / n), int(vsync_sum / n), int(copy_sum / n), int(rows_sum / n), int(fps_sum / n), n, int(prepare_sum / n), int(custom_draw_sum / n), int(cached_present_sum / n), int(overlay_present_sum / n)
  }
}
' "$ui_log"
}

parse_vsync_counters() {
  local ui_log="$1"
  awk '
function number_after(line, key, rest) {
  rest = line
  if (index(rest, key) == 0) return 0
  rest = substr(rest, index(rest, key) + length(key))
  sub(/^[^0-9]*/, "", rest)
  sub(/[^0-9].*/, "", rest)
  return rest + 0
}
/fps ~ / && /fallback=/ {
  count++
  hits[count] = number_after($0, "vsync hits=")
  fallback[count] = number_after($0, "fallback=")
  errors[count] = number_after($0, "errors=")
  if (hits[count] > 100) cumulative = 1
}
END {
  warmup = 3
  if (count == 0) {
    print 0, 0
    exit
  }
  if (cumulative) {
    base_i = (count < warmup) ? count : warmup
    fallback_total = fallback[count] - fallback[base_i]
    errors_total = errors[count] - errors[base_i]
  } else {
    for (i = warmup + 1; i <= count; i++) {
      fallback_total += fallback[i]
      errors_total += errors[i]
    }
  }
  if (fallback_total < 0) fallback_total = 0
  if (errors_total < 0) errors_total = 0
  print fallback_total + 0, errors_total + 0
}
' "$ui_log"
}

parse_mode_notes() {
  local ui_log="$1"
  awk '
function add_note(key, value) {
  if (value == "") return
  if (out != "") out = out "; "
  out = out key "=" value
}
function value_after(line, key, rest) {
  rest = line
  if (index(rest, key) == 0) return ""
  rest = substr(rest, index(rest, key) + length(key))
  sub(/[ ;].*$/, "", rest)
  return rest
}
/^slint-scale=/ {
  render = value_after($0, "render=")
  fb = value_after($0, "fb=")
}
/^slint-render-mode=/ {
  render_mode = value_after($0, "slint-render-mode=")
  frame_order = value_after($0, "frame-order=")
}
/^video_playback running / {
  render_mode = "cached"
  frame_order = value_after($0, "frame-order=")
  video_render_mode = value_after($0, "video-render-mode=")
}
/^video_render_mode=/ {
  video_render_mode = value_after($0, "video_render_mode=")
}
/^video_controls / {
  video_queue_depth = value_after($0, "queue_depth=")
  video_scale = value_after($0, "scale=")
  video_profile = value_after($0, "profile=")
  video_threads = value_after($0, "threads=")
}
/^display-config:/ {
  fb0 = value_after($0, "fb0=")
  physical = value_after($0, "uio_vres=")
  pixel_repetition = value_after($0, "pixrep=")
  uio_fb = value_after($0, "uio_fb_par=")
}
/^launcher_bench_scenario=/ {
  launcher_scenario = value_after($0, "launcher_bench_scenario=")
}
/^launcher_dirty_opt=/ {
  launcher_dirty_opt = value_after($0, "launcher_dirty_opt=")
}
END {
  add_note("physical_mode", physical)
  add_note("fb_size", fb0 != "" ? fb0 : fb)
  add_note("render_size", render)
  add_note("ui_render_mode", render_mode)
  add_note("frame_order", frame_order)
  add_note("pixel_repetition", pixel_repetition)
  add_note("uio_fb", uio_fb)
  add_note("launcher_scenario", launcher_scenario)
  add_note("launcher_dirty_opt", launcher_dirty_opt)
  add_note("video_render_mode", video_render_mode)
  add_note("video_queue_depth", video_queue_depth)
  add_note("video_scale", video_scale)
  add_note("video_profile", video_profile)
  add_note("video_threads", video_threads)
  print out
}
' "$ui_log"
}

detect_ini_mode_notes() {
  mister run "awk 'BEGIN{s=\"global\"} /^\\[/ {s=\$0; low=tolower(s)} low==\"[mister]\" || low==\"[menu]\" { if (\$0 ~ /^[[:space:]]*(video_mode|direct_video|menu_pal|forced_scandoubler|fb_size)[[:space:]]*=/) {gsub(/[ \t]+/, \"\", \$0); print s \":\" \$0} }' /media/fat/MiSTer.ini 2>/dev/null" \
    | tr '\n' ',' \
    | sed 's/,$//'
}

append_tsv_row() {
  local scene="$1" date_iso="$2"
  local prepare_us="$3" render_us="$4" custom_draw_us="$5" vsync_us="$6" copy_us="$7"
  local cached_present_us="$8" arcade_list_present_us="$9" rows_avg="${10}" fps_val="${11}"
  local cpu_mean="${12}" cpu_max="${13}" rss_kb="${14}" visual_ok="${15}" timing_ok="${16}" capture_ok="${17}" notes="${18}"
  if [[ -n "${HOST_NOTES:-}" ]]; then
    notes="${HOST_NOTES}${notes:+; }${notes}"
  fi
  notes="${notes:+$notes; }dirty_rect_broad_pct=$DIRTY_RECT_BROAD_PCT"
  notes="${notes//	/ }"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$LABEL" "$scene" "$date_iso" "$(rustc_version)" "${HOST_COMPILE_SEC:-}" "${HOST_BYTES:-}" \
    "${prepare_us:-}" "${render_us:-}" "${custom_draw_us:-}" "${vsync_us:-}" "${copy_us:-}" \
    "${cached_present_us:-}" "${arcade_list_present_us:-}" "${rows_avg:-}" "${fps_val:-}" \
    "${cpu_mean:-}" "${cpu_max:-}" "${rss_kb:-}" "$visual_ok" "$timing_ok" "$capture_ok" "$notes" >>"$TSV"
}

add_scene_failure() {
  local scene="$1" reason="$2"
  BENCH_FAILURES=$((BENCH_FAILURES + 1))
  echo "    [$scene] BENCH FAIL: $reason" >&2
}

run_self_test() {
  local tmp_dir old_tsv old_host_notes old_label old_compile old_bytes row
  tmp_dir="$(mktemp -d)"
  old_tsv="$TSV"
  old_host_notes="${HOST_NOTES:-}"
  old_label="$LABEL"
  old_compile="${HOST_COMPILE_SEC:-}"
  old_bytes="${HOST_BYTES:-}"
  trap 'rm -rf "$tmp_dir"' RETURN

  TSV="$tmp_dir/results.tsv"
  printf '%s\n' "$OLD_TSV_HEADER" >"$TSV"
  printf 'OLD\tdemo\t2026-06-21T00:00:00Z\t1.96.0\t1\t2\t3\t4\t5\t6\t60\t7\t8\t9\tyes\told-notes\n' >>"$TSV"
  normalize_results_tsv "$TSV"
  [[ "$(head -1 "$TSV")" == "$TSV_HEADER" ]] || {
    echo "self-test: TSV header normalization failed" >&2
    return 1
  }
  awk 'BEGIN { FS = "\t" }
    NR == 2 && NF == 22 && $8 == "3" && $11 == "5" && $20 == "" && $21 == "" && $22 == "old-notes" { ok = 1 }
    END { exit ok ? 0 : 1 }
  ' "$TSV" || {
    echo "self-test: old TSV row was not preserved with promoted timing columns" >&2
    return 1
  }

  TSV="$tmp_dir/split-results.tsv"
  printf '%s\n' "$SPLIT_TSV_HEADER" >"$TSV"
  printf 'SPLIT\tdemo\t2026-06-21T00:00:00Z\t1.96.0\t1\t2\t3\t4\t5\t6\t60\t7\t8\t9\tyes\tyes\tno\tsplit-notes\n' >>"$TSV"
  normalize_results_tsv "$TSV"
  awk 'BEGIN { FS = "\t" }
    NR == 2 && NF == 22 && $8 == "3" && $11 == "5" && $20 == "yes" && $21 == "no" && $22 == "split-notes" { ok = 1 }
    END { exit ok ? 0 : 1 }
  ' "$TSV" || {
    echo "self-test: split TSV row was not preserved with promoted timing columns" >&2
    return 1
  }

  TSV="$tmp_dir/overlay-present-results.tsv"
  printf '%s\n' "$OVERLAY_PRESENT_TSV_HEADER" >"$TSV"
  printf 'OVERLAY\tlauncher\t2026-06-21T00:00:00Z\t1.96.0\t1\t2\t3\t4\t5\t6\t7\t8\t9\t10\t60\t11\t12\t13\tyes\tyes\tno\toverlay-notes\n' >>"$TSV"
  normalize_results_tsv "$TSV"
  awk 'BEGIN { FS = "\t" }
    NR == 1 && $13 == "arcade_list_present_us" { header_ok = 1 }
    NR == 2 && NF == 22 && $7 == "3" && $12 == "8" && $13 == "9" && $20 == "yes" && $21 == "no" && $22 == "overlay-notes" { row_ok = 1 }
    END { exit (header_ok && row_ok) ? 0 : 1 }
  ' "$TSV" || {
    echo "self-test: overlay-present TSV row was not renamed in place" >&2
    return 1
  }

  HOST_NOTES="host-note"
  HOST_COMPILE_SEC="12"
  HOST_BYTES="34"
  LABEL="SELF"
  TSV="$tmp_dir/results.tsv"
  append_tsv_row "demo" "2026-06-21T00:01:00Z" \
    "9" "10" "11" "12" "13" "14" "15" "16" "60" "17" "18" "19" "no" "yes" "no" "capture-fail"
  row="$(tail -1 "$TSV")"
  printf '%s\n' "$row" | awk 'BEGIN { FS = "\t" }
    NF == 22 && $7 == "9" && $9 == "11" && $12 == "14" && $13 == "15" && $19 == "no" && $20 == "yes" && $21 == "no" && $22 ~ /capture-fail/ { ok = 1 }
    END { exit ok ? 0 : 1 }
  ' || {
    echo "self-test: promoted timing row was not written correctly" >&2
    return 1
  }

  cat >"$tmp_dir/cumulative-warmup.log" <<'EOF'
  fps ~ 61  | slint-render 850us  vsync-wait 15221us  fb-present 431us (314 logical rows avg)  vsync hits=60 timeouts=0 fallback=1 errors=0 hz=60.01
  fps ~ 60  | slint-render 790us  vsync-wait 15427us  fb-present 420us (310 logical rows avg)  vsync hits=120 timeouts=0 fallback=1 errors=0 hz=60.01
  fps ~ 61  | slint-render 793us  vsync-wait 15422us  fb-present 412us (310 logical rows avg)  vsync hits=181 timeouts=0 fallback=1 errors=0 hz=60.01
  fps ~ 60  | slint-render 799us  vsync-wait 15400us  fb-present 436us (310 logical rows avg)  vsync hits=241 timeouts=0 fallback=1 errors=0 hz=60.02
  fps ~ 61  | slint-render 790us  vsync-wait 15429us  fb-present 410us (310 logical rows avg)  vsync hits=302 timeouts=0 fallback=1 errors=0 hz=60.01
EOF
  [[ "$(parse_vsync_counters "$tmp_dir/cumulative-warmup.log")" == "0 0" ]] || {
    echo "self-test: cumulative warmup fallback was not ignored" >&2
    return 1
  }

  cat >"$tmp_dir/cumulative-post-warmup.log" <<'EOF'
  fps ~ 61  | slint-render 850us  vsync-wait 15221us  fb-present 431us (314 logical rows avg)  vsync hits=60 timeouts=0 fallback=0 errors=0 hz=60.01
  fps ~ 60  | slint-render 790us  vsync-wait 15427us  fb-present 420us (310 logical rows avg)  vsync hits=120 timeouts=0 fallback=0 errors=0 hz=60.01
  fps ~ 61  | slint-render 793us  vsync-wait 15422us  fb-present 412us (310 logical rows avg)  vsync hits=181 timeouts=0 fallback=0 errors=0 hz=60.01
  fps ~ 60  | slint-render 799us  vsync-wait 15400us  fb-present 436us (310 logical rows avg)  vsync hits=241 timeouts=0 fallback=1 errors=0 hz=60.02
  fps ~ 61  | slint-render 790us  vsync-wait 15429us  fb-present 410us (310 logical rows avg)  vsync hits=302 timeouts=0 fallback=1 errors=1 hz=60.01
EOF
  [[ "$(parse_vsync_counters "$tmp_dir/cumulative-post-warmup.log")" == "1 1" ]] || {
    echo "self-test: cumulative post-warmup fallback was not counted" >&2
    return 1
  }

  cat >"$tmp_dir/window-post-warmup.log" <<'EOF'
  fps ~ 61  | prepare 0us  anim 0us  slint-render 850us  custom-draw 0us  vsync-wait 15221us  fb-present 431us cached-present 0us arcade-list-present 0us (314 logical rows avg)  vsync hits=61 timeouts=0 fallback=3 errors=0
  fps ~ 60  | prepare 0us  anim 0us  slint-render 790us  custom-draw 0us  vsync-wait 15427us  fb-present 420us cached-present 0us arcade-list-present 0us (310 logical rows avg)  vsync hits=60 timeouts=0 fallback=2 errors=0
  fps ~ 61  | prepare 0us  anim 0us  slint-render 793us  custom-draw 0us  vsync-wait 15422us  fb-present 412us cached-present 0us arcade-list-present 0us (310 logical rows avg)  vsync hits=61 timeouts=0 fallback=1 errors=0
  fps ~ 60  | prepare 0us  anim 0us  slint-render 799us  custom-draw 0us  vsync-wait 15400us  fb-present 436us cached-present 0us arcade-list-present 0us (310 logical rows avg)  vsync hits=59 timeouts=1 fallback=1 errors=0
  fps ~ 61  | prepare 0us  anim 0us  slint-render 790us  custom-draw 0us  vsync-wait 15429us  fb-present 410us cached-present 0us arcade-list-present 0us (310 logical rows avg)  vsync hits=60 timeouts=0 fallback=0 errors=1
EOF
  [[ "$(parse_vsync_counters "$tmp_dir/window-post-warmup.log")" == "1 1" ]] || {
    echo "self-test: windowed post-warmup fallback was not counted" >&2
    return 1
  }

  TSV="$old_tsv"
  HOST_NOTES="$old_host_notes"
  LABEL="$old_label"
  HOST_COMPILE_SEC="$old_compile"
  HOST_BYTES="$old_bytes"
  echo "bench-toolchain self-test ok"
}

if [[ "$SELF_TEST" -eq 1 ]]; then
  HOST_COMPILE_SEC=""
  HOST_BYTES=""
  HOST_NOTES=""
  run_self_test
  exit $?
fi

normalize_results_tsv "$TSV"

if [[ "$REPLACE_LABEL" -eq 1 ]]; then
  echo "==> Removing prior rows for label=$LABEL from $TSV"
  tmp_tsv="$(mktemp)"
  awk -v label="$LABEL" 'NR == 1 || ($0 != "" && substr($0, 1, length(label) + 1) != label "\t")' "$TSV" >"$tmp_tsv"
  mv "$tmp_tsv" "$TSV"
fi

print_remote_status_summary() {
  echo "    remote status:" >&2
  mister status >&2 || true
}

preflight_fb_capture() {
  local png json
  echo "==> Preflight framebuffer capture via agent"
  mkdir -p "$HERE/build"
  png="$HERE/build/bench-fb-preflight.png"
  json="$HERE/build/bench-fb-preflight.json"
  if ! mister agent framebuffer-capture "$png" --json "$json" >/dev/null 2>&1; then
    echo "Framebuffer capture preflight failed through the MagiK agent" >&2
    print_remote_status_summary
    return 1
  fi
}

report_no_fps_lines() {
  local scene="$1" ui_log="$2"
  echo "    [$scene] no fps lines found; UI log tail follows:" >&2
  tail -40 "$ui_log" >&2 || true
  print_remote_status_summary
}

run_scene_on_device() {
  local scene="$1" secs="$2"
  local ui_log ui_full
  ui_log="$(mktemp)"
  ui_full="$(mktemp)"

  mister run "
set -e
# Visible bench path: Slint owns SPI + HDMI at 60 Hz.
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
sleep $SETTLE_SECS
if [ '$scene' = 'video_playback' ]; then
  MISTER_FRAME_ORDER=$FRAME_ORDER MISTER_DIRTY_RECT_BROAD_PCT=$DIRTY_RECT_BROAD_PCT MISTER_LAUNCHER_BENCH_SCENARIO=$LAUNCHER_SCENARIO MISTER_LAUNCHER_DIRTY_OPT=$LAUNCHER_DIRTY_OPT MISTER_VIDEO_RENDER_MODE=$VIDEO_RENDER_MODE MISTER_VIDEO_DIR='$VIDEO_REMOTE_DIR' MISTER_VIDEO_QUEUE_DEPTH=$VIDEO_QUEUE_DEPTH MISTER_VIDEO_SCALE=$VIDEO_SCALE MISTER_VIDEO_PROFILE=$VIDEO_PROFILE MISTER_VIDEO_THREADS=$VIDEO_THREADS MISTER_VIDEO_THREAD_TYPE=$VIDEO_THREAD_TYPE MISTER_VIDEO_CONVERT=$VIDEO_CONVERT $REMOTE ui $scene $secs > /tmp/bench-ui.log 2>&1 &
else
  MISTER_FRAME_ORDER=$FRAME_ORDER MISTER_DIRTY_RECT_BROAD_PCT=$DIRTY_RECT_BROAD_PCT MISTER_LAUNCHER_BENCH_SCENARIO=$LAUNCHER_SCENARIO MISTER_LAUNCHER_DIRTY_OPT=$LAUNCHER_DIRTY_OPT $REMOTE ui $scene $secs > /tmp/bench-ui.log 2>&1 &
fi
UI_PID=\$!
CPU_SUM=0
CPU_MAX=0
CPU_N=0
RSS=0
TICK=\$(getconf CLK_TCK 2>/dev/null || echo 100)
jiffies() { awk '{print \$14+\$15}' /proc/\$1/stat 2>/dev/null || echo 0; }
rss_pages() { awk '{print \$24}' /proc/\$1/stat 2>/dev/null || echo 0; }
i=0
while [ \$i -lt $secs ]; do
  if kill -0 \$UI_PID 2>/dev/null; then
    t1=\$(jiffies \$UI_PID)
    sleep 1
    t2=\$(jiffies \$UI_PID)
    p=\$(( (t2 - t1) * 100 / TICK ))
    [ \"\$p\" -lt 0 ] 2>/dev/null && p=0
    CPU_SUM=\$((CPU_SUM + p))
    [ \"\$p\" -gt \"\$CPU_MAX\" ] && CPU_MAX=\$p
    CPU_N=\$((CPU_N + 1))
    RSS=\$(rss_pages \$UI_PID)
  else
    sleep 1
  fi
  i=\$((i + 1))
done
UI_RC=0
wait \$UI_PID || UI_RC=\$?
echo ___BENCH_FB_CAPTURED___
echo agent
echo ___BENCH_SCENE___
echo $scene
echo ___BENCH_CPU_MEAN___
if [ \$CPU_N -gt 0 ]; then echo \$((CPU_SUM / CPU_N)); else echo 0; fi
echo ___BENCH_CPU_MAX___
echo \$CPU_MAX
echo ___BENCH_RSS___
echo \$RSS
echo ___BENCH_UI_RC___
echo \$UI_RC
echo ___BENCH_UI_LOG___
cat /tmp/bench-ui.log
" >"$ui_full" 2>&1 || true

  if grep -q ___BENCH_UI_LOG___ "$ui_full"; then
    sed -n '/___BENCH_UI_LOG___/,$p' "$ui_full" | tail -n +2 >"$ui_log"
  else
    cp "$ui_full" "$ui_log"
  fi
  cp "$ui_log" "$BENCH_DIR/${LABEL}-${scene}-ui.log" 2>/dev/null || true

  local cpu_mean cpu_max rss_kb ui_rc parse_stats
  cpu_mean="$(sed -n '/___BENCH_CPU_MEAN___/{n;p;}' "$ui_full" 2>/dev/null | head -1)"
  cpu_max="$(sed -n '/___BENCH_CPU_MAX___/{n;p;}' "$ui_full" 2>/dev/null | head -1)"
  rss_kb="$(sed -n '/___BENCH_RSS___/{n;p;}' "$ui_full" 2>/dev/null | head -1)"
  ui_rc="$(sed -n '/___BENCH_UI_RC___/{n;p;}' "$ui_full" 2>/dev/null | head -1)"
  if [[ -n "$rss_kb" && "$rss_kb" =~ ^[0-9]+$ && "$rss_kb" -lt 100000 ]]; then
    rss_kb=$((rss_kb * 4))
  fi

  local render_us="" vsync_us="" copy_us="" rows_avg="" fps_val="" visual_ok="no" timing_ok="yes" capture_ok="no" notes="" mode_notes=""
  local prepare_us="" custom_draw_us="" cached_present_us="" arcade_list_present_us=""
  local scene_failures=""
  parse_stats="$(parse_ui_log "$ui_log")" || true
  if [[ -n "$parse_stats" ]]; then
    read -r render_us vsync_us copy_us rows_avg fps_val _cnt prepare_us custom_draw_us cached_present_us arcade_list_present_us <<<"$parse_stats"
  else
    timing_ok="no"
    notes="no-fps-lines"
    scene_failures="${scene_failures:+$scene_failures,}no-fps-lines"
    report_no_fps_lines "$scene" "$ui_log"
  fi
  mode_notes="$(parse_mode_notes "$ui_log")"
  if [[ -n "$mode_notes" ]]; then
    notes="${notes:+$notes; }$mode_notes"
  fi

  local vsync_fallback="" vsync_errors="" vsync_stats=""
  vsync_stats="$(parse_vsync_counters "$ui_log")" || true
  if [[ -n "$vsync_stats" ]]; then
    read -r vsync_fallback vsync_errors <<<"$vsync_stats"
    notes="${notes:+$notes; }vsync_fallback=${vsync_fallback:-0}; vsync_errors=${vsync_errors:-0}"
  fi

  if [[ "${ui_rc:-1}" == "0" ]] || grep -q '^done:' "$ui_log"; then
    :
  else
    timing_ok="no"
    notes="${notes:+$notes; }ui-rc=${ui_rc:-?}"
    scene_failures="${scene_failures:+$scene_failures,}ui-rc=${ui_rc:-?}"
  fi

  if [[ -n "$fps_val" && "$fps_val" =~ ^[0-9]+$ && "$fps_val" -lt "$MIN_FPS" ]]; then
    timing_ok="no"
    scene_failures="${scene_failures:+$scene_failures,}fps=${fps_val}<${MIN_FPS}"
  fi
  if [[ -n "$vsync_fallback" && "$vsync_fallback" =~ ^[0-9]+$ && "$vsync_fallback" -gt "$MAX_VSYNC_FALLBACK" ]]; then
    timing_ok="no"
    scene_failures="${scene_failures:+$scene_failures,}vsync-fallback=${vsync_fallback}>${MAX_VSYNC_FALLBACK}"
  fi
  if [[ -n "$vsync_errors" && "$vsync_errors" =~ ^[0-9]+$ && "$vsync_errors" -gt "$MAX_VSYNC_ERRORS" ]]; then
    timing_ok="no"
    scene_failures="${scene_failures:+$scene_failures,}vsync-errors=${vsync_errors}>${MAX_VSYNC_ERRORS}"
  fi
  if [[ -n "$MAX_RENDER_US" && -n "$render_us" && "$render_us" =~ ^[0-9]+$ && "$render_us" -gt "$MAX_RENDER_US" ]]; then
    timing_ok="no"
    scene_failures="${scene_failures:+$scene_failures,}render_us=${render_us}>${MAX_RENDER_US}"
  fi
  if [[ -n "$MAX_COPY_US" && -n "$copy_us" && "$copy_us" =~ ^[0-9]+$ && "$copy_us" -gt "$MAX_COPY_US" ]]; then
    timing_ok="no"
    scene_failures="${scene_failures:+$scene_failures,}copy_us=${copy_us}>${MAX_COPY_US}"
  fi

  echo "    [$scene] slint-render=${render_us:-?}us fb-present=${copy_us:-?}us rows=${rows_avg:-?} cpu_mean=${cpu_mean:-?}%"

  local png_out="$BENCH_DIR/${LABEL}-${scene}-fb.png"
  echo "==> Capture $png_out via agent"
  if mister agent framebuffer-capture "$png_out" --json "${png_out%.png}.json" >/dev/null 2>&1; then
    capture_ok="yes"
  else
    capture_ok="no"
    notes="${notes:+$notes; }capture-fail"
    scene_failures="${scene_failures:+$scene_failures,}capture-fail"
    echo "    agent framebuffer capture failed" >&2
  fi

  if [[ "$timing_ok" == "yes" && "$capture_ok" == "yes" ]]; then
    visual_ok="yes"
  fi
  if [[ "$timing_ok" != "yes" ]]; then
    scene_failures="${scene_failures:+$scene_failures,}timing_ok=$timing_ok"
  fi
  if [[ "$capture_ok" != "yes" ]]; then
    scene_failures="${scene_failures:+$scene_failures,}capture_ok=$capture_ok"
  fi
  if [[ "$visual_ok" != "yes" ]]; then
    scene_failures="${scene_failures:+$scene_failures,}visual_ok=$visual_ok"
  fi
  if [[ -n "$scene_failures" ]]; then
    notes="${notes:+$notes; }bench_failures=$scene_failures"
    add_scene_failure "$scene" "$scene_failures"
  fi

  append_tsv_row "$scene" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    "$prepare_us" "$render_us" "$custom_draw_us" "$vsync_us" "$copy_us" \
    "$cached_present_us" "$arcade_list_present_us" "$rows_avg" "$fps_val" \
    "$cpu_mean" "$cpu_max" "$rss_kb" "$visual_ok" "$timing_ok" "$capture_ok" "$notes"

  rm -f "$ui_log" "$ui_full"
}

echo "==> Toolchain bench label=$LABEL scenes=${BENCH_SCENES[*]} (${SCENE_SECS}s each)"

HOST_COMPILE_SEC=""
HOST_BYTES=""
HOST_NOTES=""
rustc_ver="$(rustc_version)"

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  if [[ "$DO_CLEAN" -eq 1 ]]; then
    echo "==> cargo clean"
    (cd "$RUST_DIR" && cargo clean)
  fi
  echo "==> Cross-build (timed)"
  build_log="$(mktemp)"
  HOST_COMPILE_SEC="$( ( time -p env MISTER_UI_BUILD_SCOPE="$UI_SCOPE" "$RUST_DIR/build-arm.sh" "${BUILD_FLAG[@]}" ) 2>&1 | tee "$build_log" | awk '/^real /{print $2}')"
  HOST_NOTES="profile=$BUILD_PROFILE; ui_scope=$UI_SCOPE; prep=suspend-main-ui; design=runtime; render=runtime; font=PressStart2P; fpga-scale-ui=960x540-to-1920x1080"
  rm -f "$build_log"
  [[ -f "$BIN" ]] || { echo "Build failed: missing $BIN" >&2; exit 1; }
else
  HOST_NOTES="skip-build; profile=$BUILD_PROFILE; ui_scope=$UI_SCOPE; prep=suspend-main-ui; design=runtime; render=runtime; font=PressStart2P; fpga-scale-ui=960x540-to-1920x1080"
  [[ -f "$BIN" ]] || { echo "No binary at $BIN" >&2; exit 1; }
fi

if stat -f%z "$BIN" >/dev/null 2>&1; then
  HOST_BYTES="$(stat -f%z "$BIN")"
else
  HOST_BYTES="$(stat -c%s "$BIN")"
fi
echo "    rustc=$rustc_ver  compile_sec=${HOST_COMPILE_SEC:-n/a}  bytes=$HOST_BYTES"

if [[ "$SKIP_DEVICE" -eq 0 ]]; then
  echo "==> Deploy $BIN"
  mister_suspend_launcher
  trap 'mister_restart_launcher >/dev/null 2>&1 || true' EXIT
  mister run "kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true; mkdir -p /media/fat/mister-magik"
  mister put "$BIN" "$REMOTE"
  mister run "chmod +x $REMOTE"
  if [[ "$INCLUDE_VIDEO" -eq 1 ]]; then
    echo "==> Sync video snaps $VIDEO_SRC_DIR -> $VIDEO_REMOTE_DIR"
    "$HERE/scripts/sync-video-snaps.sh" "$VIDEO_SRC_DIR" "$VIDEO_REMOTE_DIR"
  fi
  mister run "file $REMOTE && ldd $REMOTE 2>&1 | head -3" || HOST_NOTES="${HOST_NOTES:+$HOST_NOTES; }ldd-fail"
  ini_mode_notes="$(detect_ini_mode_notes || true)"
  if [[ -n "$ini_mode_notes" ]]; then
    HOST_NOTES="${HOST_NOTES:+$HOST_NOTES; }ini_mode=$ini_mode_notes"
  fi
  preflight_fb_capture

  for scene in "${BENCH_SCENES[@]}"; do
    echo "==> Scene $scene (${SCENE_SECS}s)"
    run_scene_on_device "$scene" "$SCENE_SECS"
  done
else
  date_iso="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  for scene in "${BENCH_SCENES[@]}"; do
    append_tsv_row "$scene" "$date_iso" "" "" "" "" "" "" "" "" "" "n/a" "n/a" "n/a" "n/a" "n/a" "n/a" "${HOST_NOTES:+$HOST_NOTES; }skip-device"
  done
fi

echo "==> Results: $TSV"
echo ""
column -t -s $'\t' "$TSV" 2>/dev/null || cat "$TSV"

if [[ "$BENCH_FAILURES" -gt 0 ]]; then
  echo "==> Benchmark gate failed: $BENCH_FAILURES scene(s) exceeded thresholds" >&2
  exit 1
fi
