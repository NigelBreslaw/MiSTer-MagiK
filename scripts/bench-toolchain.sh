#!/usr/bin/env bash
# Benchmark a toolchain configuration: host compile time + binary size, then each
# Slint bench scene on the MiSTer (timings, CPU, framebuffer PNG).
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-toolchain.sh A0 --clean
#
# Appends one row per scene to history/toolchain-bench/results.tsv.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$HERE/magik-gui"
BUILD_PROFILE=release
BUILD_FLAG=()
REMOTE="/media/fat/mister-magik/mister-magik-fb"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results.tsv"
MISTER="$HERE/scripts/mister"

# Slint scenes (see magik-gui/ui/bench/README.md)
BENCH_SCENES=(demo full_motion static_ui local_motion console_scroll)
VIDEO_SRC="${MISTER_VIDEO_SRC:-$HERE/build/video/mslug3_320x224_60_h264_baseline_pcm_s16le_mono.mov}"
VIDEO_REMOTE="${MISTER_VIDEO_REMOTE:-/media/fat/mister-magik/mslug3.mov}"

export MISTER_IP="${MISTER_IP:-192.168.1.117}"
export MISTER_PASS="${MISTER_PASS:-1}"
export RUSTC_WRAPPER=""

LABEL="A0"
DO_CLEAN=0
SKIP_BUILD=0
SKIP_DEVICE=0
REPLACE_LABEL=0
INCLUDE_VIDEO=0
SCENE_FILTER=0
SCENE_SECS=15
SETTLE_SECS="${MISTER_BENCH_SETTLE_SECS:-5}"
FRAME_ORDER="${MISTER_FRAME_ORDER:-render-then-vsync}"
DIRTY_RECT_BROAD_PCT="${MISTER_DIRTY_RECT_BROAD_PCT:-85}"
LAUNCHER_SCENARIO="${MISTER_LAUNCHER_BENCH_SCENARIO:-}"
LAUNCHER_DIRTY_OPT="${MISTER_LAUNCHER_DIRTY_OPT:-on}"
VIDEO_RENDER_MODE="${MISTER_VIDEO_RENDER_MODE:-slint-image}"
UI_SCOPE="${MISTER_UI_BUILD_SCOPE:-all}"
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
  echo "         --device (build profile release-device / A3)  --video  --scene NAME  -h"
  echo "         --frame-order render-then-vsync|vsync-first"
  echo "         --dirty-rect-broad-pct N"
  echo "         --launcher-scenario idle|home-nav|list-scroll|quick-tap|rapid-taps|held-scroll|turbo-hold|preview-changes"
  echo "         --launcher-dirty-opt on|off"
  echo "         --video-render-mode slint-image|direct-blit"
  echo "         --ui-scope all|launcher|arcade"
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
    --device) BUILD_PROFILE=release-device; BUILD_FLAG+=(--device); shift ;;
    --video) INCLUDE_VIDEO=1; BUILD_FLAG+=(--video); shift ;;
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
    idle|home-nav|list-scroll|quick-tap|rapid-taps|held-scroll|turbo-hold|preview-changes) ;;
    *) echo "Unknown --launcher-scenario: $LAUNCHER_SCENARIO" >&2; exit 1 ;;
  esac
fi
case "$LAUNCHER_DIRTY_OPT" in
  on|off|1|0|true|false) ;;
  *) echo "Unknown --launcher-dirty-opt: $LAUNCHER_DIRTY_OPT" >&2; exit 1 ;;
esac
case "$VIDEO_RENDER_MODE" in
  slint-image|direct-blit) ;;
  *) echo "Unknown --video-render-mode: $VIDEO_RENDER_MODE" >&2; exit 1 ;;
esac
case "$UI_SCOPE" in
  all|launcher|arcade) ;;
  *) echo "Unknown --ui-scope: $UI_SCOPE (use all|launcher|arcade)" >&2; exit 1 ;;
esac
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

TSV_HEADER="label	scene	date	rustc	compile_sec	bytes	render_us	vsync_us	copy_us	rows_avg	fps	cpu_mean	cpu_max	rss_kb	visual_ok	notes"
if [[ ! -f "$TSV" ]] || ! head -1 "$TSV" | grep -q $'^label\tscene'; then
  echo "$TSV_HEADER" >"$TSV"
fi

if [[ "$REPLACE_LABEL" -eq 1 ]]; then
  echo "==> Removing prior rows for label=$LABEL from $TSV"
  tmp_tsv="$(mktemp)"
  awk -v label="$LABEL" 'NR == 1 || ($0 != "" && substr($0, 1, length(label) + 1) != label "\t")' "$TSV" >"$tmp_tsv"
  mv "$tmp_tsv" "$TSV"
fi

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
    overlay_present = number_after($0, "overlay-present ")
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
/fallback=/ {
  fallback += number_after($0, "fallback=")
}
/errors=/ {
  errors += number_after($0, "errors=")
}
END {
  print fallback + 0, errors + 0
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
  fb_scale = value_after($0, "fb_scale=")
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
  add_note("fb_scale", fb_scale)
  add_note("ui_render_mode", render_mode)
  add_note("frame_order", frame_order)
  add_note("pixel_repetition", pixel_repetition)
  add_note("uio_fb", uio_fb)
  add_note("launcher_scenario", launcher_scenario)
  add_note("launcher_dirty_opt", launcher_dirty_opt)
  add_note("video_render_mode", video_render_mode)
  print out
}
' "$ui_log"
}

capture_size_from_log() {
  local ui_log="$1"
  local size
  size="$(sed -n 's/^slint-scale=.* fb=\([0-9][0-9]*\)x\([0-9][0-9]*\).*/\1 \2/p' "$ui_log" | head -1)"
  if [[ -z "$size" ]]; then
    size="$(sed -n 's/^display-config: .*fb0=\([0-9][0-9]*\)x\([0-9][0-9]*\).*/\1 \2/p' "$ui_log" | head -1)"
  fi
  echo "$size"
}

detect_ini_mode_notes() {
  mister run "awk 'BEGIN{s=\"global\"} /^\\[/ {s=\$0; low=tolower(s)} low==\"[mister]\" || low==\"[menu]\" { if (\$0 ~ /^[[:space:]]*(video_mode|direct_video|menu_pal|forced_scandoubler|fb_size)[[:space:]]*=/) {gsub(/[ \t]+/, \"\", \$0); print s \":\" \$0} }' /media/fat/MiSTer.ini 2>/dev/null" \
    | tr '\n' ',' \
    | sed 's/,$//'
}

append_tsv_row() {
  local scene="$1" date_iso="$2"
  local render_us="$3" vsync_us="$4" copy_us="$5" rows_avg="$6" fps_val="$7"
  local cpu_mean="$8" cpu_max="$9" rss_kb="${10}" visual_ok="${11}" notes="${12}"
  if [[ -n "${HOST_NOTES:-}" ]]; then
    notes="${HOST_NOTES}${notes:+; }${notes}"
  fi
  notes="${notes:+$notes; }dirty_rect_broad_pct=$DIRTY_RECT_BROAD_PCT"
  notes="${notes//	/ }"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$LABEL" "$scene" "$date_iso" "$(rustc_version)" "${HOST_COMPILE_SEC:-}" "${HOST_BYTES:-}" \
    "${render_us:-}" "${vsync_us:-}" "${copy_us:-}" "${rows_avg:-}" "${fps_val:-}" \
    "${cpu_mean:-}" "${cpu_max:-}" "${rss_kb:-}" "$visual_ok" "$notes" >>"$TSV"
}

add_scene_failure() {
  local scene="$1" reason="$2"
  BENCH_FAILURES=$((BENCH_FAILURES + 1))
  echo "    [$scene] BENCH FAIL: $reason" >&2
}

run_scene_on_device() {
  local scene="$1" secs="$2"
  local ui_log ui_full
  ui_log="$(mktemp)"
  ui_full="$(mktemp)"
  # Snapshot /dev/fb0 while the UI process is still running.
  # Post-exit capture only sees fbcon "Welcome / login:" — not the bench scene.
  local capture_at=$((secs > 4 ? secs - 2 : 2))

  mister run "
set -e
# Visible bench path: Slint owns SPI + HDMI at 60 Hz.
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
kill -9 \$(pidof MiSTer_MagiK) 2>/dev/null || true
kill -9 \$(pidof MiSTer) 2>/dev/null || true
sleep $SETTLE_SECS
MISTER_FRAME_ORDER=$FRAME_ORDER MISTER_DIRTY_RECT_BROAD_PCT=$DIRTY_RECT_BROAD_PCT MISTER_LAUNCHER_BENCH_SCENARIO=$LAUNCHER_SCENARIO MISTER_LAUNCHER_DIRTY_OPT=$LAUNCHER_DIRTY_OPT MISTER_VIDEO_RENDER_MODE=$VIDEO_RENDER_MODE $REMOTE ui $scene $secs > /tmp/bench-ui.log 2>&1 &
UI_PID=\$!
CPU_SUM=0
CPU_MAX=0
CPU_N=0
RSS=0
FB_CAPTURED=0
TICK=\$(getconf CLK_TCK 2>/dev/null || echo 100)
jiffies() { awk '{print \$14+\$15}' /proc/\$1/stat 2>/dev/null || echo 0; }
rss_pages() { awk '{print \$24}' /proc/\$1/stat 2>/dev/null || echo 0; }
i=0
while [ \$i -lt $secs ]; do
  if kill -0 \$UI_PID 2>/dev/null; then
    if [ \$FB_CAPTURED -eq 0 ] && [ \$i -ge $capture_at ]; then
      dd if=/dev/fb0 of=/tmp/bench-fb.raw bs=1M count=32 2>/dev/null && FB_CAPTURED=1
    fi
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
wait \$UI_PID
UI_RC=\$?
echo ___BENCH_FB_CAPTURED___
echo \$FB_CAPTURED
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

  local render_us="" vsync_us="" copy_us="" rows_avg="" fps_val="" visual_ok="no" notes="" mode_notes=""
  local prepare_us="" custom_draw_us="" cached_present_us="" overlay_present_us=""
  local scene_failures=""
  parse_stats="$(parse_ui_log "$ui_log")" || true
  if [[ -n "$parse_stats" ]]; then
    read -r render_us vsync_us copy_us rows_avg fps_val _cnt prepare_us custom_draw_us cached_present_us overlay_present_us <<<"$parse_stats"
    notes="${notes:+$notes; }prepare_us=${prepare_us:-0}; custom_draw_us=${custom_draw_us:-0}; cached_present_us=${cached_present_us:-0}; overlay_present_us=${overlay_present_us:-0}"
  else
    notes="no-fps-lines"
    scene_failures="${scene_failures:+$scene_failures,}no-fps-lines"
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
    visual_ok="yes"
  else
    visual_ok="no"
    notes="${notes:+$notes; }ui-rc=${ui_rc:-?}"
    scene_failures="${scene_failures:+$scene_failures,}ui-rc=${ui_rc:-?}"
  fi

  if [[ -n "$fps_val" && "$fps_val" =~ ^[0-9]+$ && "$fps_val" -lt "$MIN_FPS" ]]; then
    scene_failures="${scene_failures:+$scene_failures,}fps=${fps_val}<${MIN_FPS}"
  fi
  if [[ -n "$vsync_fallback" && "$vsync_fallback" =~ ^[0-9]+$ && "$vsync_fallback" -gt "$MAX_VSYNC_FALLBACK" ]]; then
    scene_failures="${scene_failures:+$scene_failures,}vsync-fallback=${vsync_fallback}>${MAX_VSYNC_FALLBACK}"
  fi
  if [[ -n "$vsync_errors" && "$vsync_errors" =~ ^[0-9]+$ && "$vsync_errors" -gt "$MAX_VSYNC_ERRORS" ]]; then
    scene_failures="${scene_failures:+$scene_failures,}vsync-errors=${vsync_errors}>${MAX_VSYNC_ERRORS}"
  fi
  if [[ -n "$MAX_RENDER_US" && -n "$render_us" && "$render_us" =~ ^[0-9]+$ && "$render_us" -gt "$MAX_RENDER_US" ]]; then
    scene_failures="${scene_failures:+$scene_failures,}render_us=${render_us}>${MAX_RENDER_US}"
  fi
  if [[ -n "$MAX_COPY_US" && -n "$copy_us" && "$copy_us" =~ ^[0-9]+$ && "$copy_us" -gt "$MAX_COPY_US" ]]; then
    scene_failures="${scene_failures:+$scene_failures,}copy_us=${copy_us}>${MAX_COPY_US}"
  fi

  echo "    [$scene] slint-render=${render_us:-?}us fb-present=${copy_us:-?}us rows=${rows_avg:-?} cpu_mean=${cpu_mean:-?}%"

  local fb_captured png_out="$BENCH_DIR/${LABEL}-${scene}-fb.png"
  fb_captured="$(sed -n '/___BENCH_FB_CAPTURED___/{n;p;}' "$ui_full" 2>/dev/null | head -1)"
  echo "==> Capture $png_out (mid-run snapshot at ~${capture_at}s)"
  mkdir -p "$HERE/build"
  local raw="$HERE/build/bench-fb.raw"
  local capture_size capture_w capture_h
  capture_size="$(capture_size_from_log "$ui_log")"
  if [[ -n "$capture_size" ]]; then
    read -r capture_w capture_h <<<"$capture_size"
  else
    capture_w=1920
    capture_h=1080
    notes="${notes:+$notes; }capture-size-fallback=1920x1080"
  fi
  if [[ "$fb_captured" == "1" ]] && mister get /tmp/bench-fb.raw "$raw" >/dev/null 2>&1 \
    && mister raw-to-png "$raw" "$capture_w" "$capture_h" "$png_out" >/dev/null 2>&1; then
    :
  else
    visual_ok="no"
    notes="${notes:+$notes; }capture-fail"
    scene_failures="${scene_failures:+$scene_failures,}capture-fail"
    echo "    capture failed (fb_captured=${fb_captured:-?})" >&2
  fi

  if [[ "$visual_ok" != "yes" ]]; then
    scene_failures="${scene_failures:+$scene_failures,}visual_ok=$visual_ok"
  fi
  if [[ -n "$scene_failures" ]]; then
    notes="${notes:+$notes; }bench_failures=$scene_failures"
    add_scene_failure "$scene" "$scene_failures"
  fi

  append_tsv_row "$scene" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    "$render_us" "$vsync_us" "$copy_us" "$rows_avg" "$fps_val" \
    "$cpu_mean" "$cpu_max" "$rss_kb" "$visual_ok" "$notes"

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
  HOST_NOTES="profile=$BUILD_PROFILE; ui_scope=$UI_SCOPE; prep=kill-mister-ui; design=runtime; render=runtime; font=PressStart2P; fpga-scale-ui=960x540-to-1920x1080"
  rm -f "$build_log"
  [[ -f "$BIN" ]] || { echo "Build failed: missing $BIN" >&2; exit 1; }
else
  HOST_NOTES="skip-build; profile=$BUILD_PROFILE; ui_scope=$UI_SCOPE; prep=kill-mister-ui; design=runtime; render=runtime; font=PressStart2P; fpga-scale-ui=960x540-to-1920x1080"
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
  mister run "kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true; mkdir -p /media/fat/mister-magik"
  mister put "$BIN" "$REMOTE"
  mister run "chmod +x $REMOTE"
  if [[ "$INCLUDE_VIDEO" -eq 1 ]]; then
    if [[ ! -f "$VIDEO_SRC" ]]; then
      echo "Video benchmark source missing: $VIDEO_SRC" >&2
      exit 1
    fi
    echo "==> Deploy video $VIDEO_SRC -> $VIDEO_REMOTE"
    mister put "$VIDEO_SRC" "$VIDEO_REMOTE"
  fi
  mister run "file $REMOTE && ldd $REMOTE 2>&1 | head -3" || HOST_NOTES="${HOST_NOTES:+$HOST_NOTES; }ldd-fail"
  ini_mode_notes="$(detect_ini_mode_notes || true)"
  if [[ -n "$ini_mode_notes" ]]; then
    HOST_NOTES="${HOST_NOTES:+$HOST_NOTES; }ini_mode=$ini_mode_notes"
  fi

  for scene in "${BENCH_SCENES[@]}"; do
    echo "==> Scene $scene (${SCENE_SECS}s)"
    run_scene_on_device "$scene" "$SCENE_SECS"
  done
else
  date_iso="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  for scene in "${BENCH_SCENES[@]}"; do
    append_tsv_row "$scene" "$date_iso" "" "" "" "" "" "" "" "" "n/a" "${HOST_NOTES:+$HOST_NOTES; }skip-device"
  done
fi

echo "==> Results: $TSV"
echo ""
column -t -s $'\t' "$TSV" 2>/dev/null || cat "$TSV"

if [[ "$BENCH_FAILURES" -gt 0 ]]; then
  echo "==> Benchmark gate failed: $BENCH_FAILURES scene(s) exceeded thresholds" >&2
  exit 1
fi
