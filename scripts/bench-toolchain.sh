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
BENCH_SCENES=(demo full_motion static_ui local_motion text_heavy solid_fill list_scroll console_scroll dirty_band)
VIDEO_SRC="${MISTER_VIDEO_SRC:-$HERE/build/video/mslug3_320x224_60_h264_baseline_pcm_s16le_mono.mov}"
VIDEO_REMOTE="${MISTER_VIDEO_REMOTE:-/media/fat/mister-magik/mslug3.mov}"

export MISTER_IP="${MISTER_IP:-192.168.1.117}"
export MISTER_PASS="${MISTER_PASS:-1}"

LABEL="A0"
RENDER_SCALE="${MISTER_RENDER_SCALE:-}"
# Legacy alias (downscale-from-HDMI): PIXEL_SCALE=2 ≡ RENDER_SCALE=1.
PIXEL_SCALE="${MISTER_PIXEL_SCALE:-}"
DO_CLEAN=0
SKIP_BUILD=0
SKIP_DEVICE=0
REPLACE_LABEL=0
INCLUDE_VIDEO=0
SCENE_FILTER=0
SCENE_SECS=15
SETTLE_SECS="${MISTER_BENCH_SETTLE_SECS:-5}"

usage() {
  sed -n '2,22p' "$0" | sed 's/^# \{0,1\}//'
  echo ""
  echo "Scenes: ${BENCH_SCENES[*]}"
  echo ""
  echo "Options: --clean  --skip-build  --skip-device  --replace-label  --scene-secs N"
  echo "         --device (build profile release-device / A3)  --video  --scene NAME  -h"
  echo "  (--ui-secs N is an alias for --scene-secs)"
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
    -*) echo "Unknown option: $1" >&2; usage 1 ;;
    *) LABEL="$1"; shift ;;
  esac
done

if [[ "$INCLUDE_VIDEO" -eq 1 && "$SCENE_FILTER" -eq 0 ]]; then
  BENCH_SCENES+=(video_playback)
fi

# Label defaults: P2* → half-res render (960×540); A*/PS/LS → full-res (1920×1080).
if [[ -z "$RENDER_SCALE" ]]; then
  case "$LABEL" in
    P2*|p2*) RENDER_SCALE=1 ;;
    A*|PS|LS) RENDER_SCALE=2 ;;
    *) RENDER_SCALE=1 ;;
  esac
fi
# Legacy MISTER_PIXEL_SCALE overrides when RENDER_SCALE was not set explicitly.
if [[ -z "${MISTER_RENDER_SCALE:-}" && -n "$PIXEL_SCALE" ]]; then
  case "$PIXEL_SCALE" in
    1) RENDER_SCALE=2 ;;
    2) RENDER_SCALE=1 ;;
  esac
fi

BIN="$RUST_DIR/target/armv7-unknown-linux-gnueabihf/$BUILD_PROFILE/mister-magik-fb"

mkdir -p "$BENCH_DIR"

TSV_HEADER="label	scene	date	rustc	compile_sec	bytes	render_us	vsync_us	copy_us	rows_avg	fps	cpu_mean	cpu_max	rss_kb	visual_ok	notes"
if [[ ! -f "$TSV" ]] || ! head -1 "$TSV" | grep -q $'^label\tscene'; then
  echo "$TSV_HEADER" >"$TSV"
fi

if [[ "$REPLACE_LABEL" -eq 1 ]]; then
  echo "==> Removing prior rows for label=$LABEL from $TSV"
  LABEL="$LABEL" TSV="$TSV" python3 <<'PY'
import os
path = os.environ["TSV"]
label = os.environ["LABEL"]
with open(path, encoding="utf-8") as f:
    lines = f.readlines()
if not lines:
    raise SystemExit(0)
header = lines[0]
rows = [ln for ln in lines[1:] if ln.strip() and not ln.startswith(label + "\t")]
with open(path, "w", encoding="utf-8") as f:
    f.write(header)
    f.writelines(rows)
PY
fi

mister() {
  "$MISTER" "$@"
}

rustc_version() {
  (cd "$RUST_DIR" && rustc -V 2>/dev/null | awk '{print $2}')
}

parse_ui_log() {
  local ui_log="$1"
  UI_LOG="$ui_log" python3 <<'PY'
import os, re, sys
path = os.environ["UI_LOG"]
pat = re.compile(
    r"fps ~ (\d+).*render (\d+)us.*vsync-wait (\d+)us.*copy (\d+)us.*\((\d+) (?:logical )?rows avg\)"
)
console_pat = re.compile(
    r"fps ~ (\d+).*ram-scroll (\d+)us.*exposed-strip (\d+)us.*fb-copy (\d+)us"
)
rows = []
with open(path, encoding="utf-8", errors="ignore") as f:
    for line in f:
        m = pat.search(line)
        if m:
            rows.append(tuple(int(m.group(i)) for i in range(1, 6)))
            continue
        m = console_pat.search(line)
        if m:
            rows.append((
                int(m.group(1)),
                int(m.group(2)),
                int(m.group(3)),
                int(m.group(4)),
                0,
            ))
if len(rows) <= 3:
    sys.exit(0)
rows = rows[3:]
n = len(rows)
print(
    sum(r[1] for r in rows) // n,
    sum(r[2] for r in rows) // n,
    sum(r[3] for r in rows) // n,
    sum(r[4] for r in rows) // n,
    sum(r[0] for r in rows) // n,
    n,
)
PY
}

append_tsv_row() {
  local scene="$1" date_iso="$2"
  local render_us="$3" vsync_us="$4" copy_us="$5" rows_avg="$6" fps_val="$7"
  local cpu_mean="$8" cpu_max="$9" rss_kb="${10}" visual_ok="${11}" notes="${12}"
  if [[ -n "${HOST_NOTES:-}" ]]; then
    notes="${HOST_NOTES}${notes:+; }${notes}"
  fi
  notes="${notes//	/ }"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$LABEL" "$scene" "$date_iso" "$(rustc_version)" "${HOST_COMPILE_SEC:-}" "${HOST_BYTES:-}" \
    "${render_us:-}" "${vsync_us:-}" "${copy_us:-}" "${rows_avg:-}" "${fps_val:-}" \
    "${cpu_mean:-}" "${cpu_max:-}" "${rss_kb:-}" "$visual_ok" "$notes" >>"$TSV"
}

run_scene_on_device() {
  local scene="$1" secs="$2"
  local ui_log ui_full
  ui_log="$(mktemp)"
  ui_full="$(mktemp)"
  # Snapshot /dev/fb0 while the UI process is still running.
  # Post-exit capture only sees fbcon "Welcome / login:" — not the bench scene.
  local capture_at=$((secs > 4 ? secs - 2 : 2))

  local render_env=""
  if [[ -n "$RENDER_SCALE" ]]; then
    render_env="MISTER_RENDER_SCALE=$RENDER_SCALE "
  fi
  mister run "
set -e
# Visible bench path: Slint owns SPI + HDMI at 60 Hz (see scripts/bench-diagnose.sh).
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
kill -9 \$(pidof MiSTer_MagiK) 2>/dev/null || true
kill -9 \$(pidof MiSTer) 2>/dev/null || true
sleep $SETTLE_SECS
${render_env}$REMOTE ui $scene $secs > /tmp/bench-ui.log 2>&1 &
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
      dd if=/dev/fb0 of=/tmp/bench-fb.raw bs=1M count=8 2>/dev/null && FB_CAPTURED=1
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

  local render_us="" vsync_us="" copy_us="" rows_avg="" fps_val="" visual_ok="no" notes=""
  parse_stats="$(parse_ui_log "$ui_log")" || true
  if [[ -n "$parse_stats" ]]; then
    read -r render_us vsync_us copy_us rows_avg fps_val _cnt <<<"$parse_stats"
    if [[ "$scene" == "console_scroll" ]]; then
      notes="${notes:+$notes; }console_scroll: render_us=ram-scroll; vsync_us=exposed-strip; copy_us=fb-copy"
    fi
  else
    notes="no-fps-lines"
  fi

  if [[ "${ui_rc:-1}" == "0" ]] || grep -q '^done:' "$ui_log"; then
    visual_ok="yes"
  else
    visual_ok="no"
    notes="${notes:+$notes; }ui-rc=${ui_rc:-?}"
  fi

  echo "    [$scene] render=${render_us:-?}us copy=${copy_us:-?}us rows=${rows_avg:-?} cpu_mean=${cpu_mean:-?}%"

  local fb_captured png_out="$BENCH_DIR/${LABEL}-${scene}-fb.png"
  fb_captured="$(sed -n '/___BENCH_FB_CAPTURED___/{n;p;}' "$ui_full" 2>/dev/null | head -1)"
  echo "==> Capture $png_out (mid-run snapshot at ~${capture_at}s)"
  mkdir -p "$HERE/build"
  local raw="$HERE/build/bench-fb.raw"
  if [[ "$fb_captured" == "1" ]] && mister get /tmp/bench-fb.raw "$raw" >/dev/null 2>&1 \
    && python3 "$HERE/scripts/raw_to_png.py" "$raw" 1920 1080 "$png_out" >/dev/null 2>&1; then
    :
  else
    visual_ok="no"
    notes="${notes:+$notes; }capture-fail"
    echo "    capture failed (fb_captured=${fb_captured:-?})" >&2
  fi

  append_tsv_row "$scene" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    "$render_us" "$vsync_us" "$copy_us" "$rows_avg" "$fps_val" \
    "$cpu_mean" "$cpu_max" "$rss_kb" "$visual_ok" "$notes"

  rm -f "$ui_log" "$ui_full"
}

echo "==> Toolchain bench label=$LABEL render_scale=$RENDER_SCALE scenes=${BENCH_SCENES[*]} (${SCENE_SECS}s each)"

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
  HOST_COMPILE_SEC="$( ( time -p "$RUST_DIR/build-arm.sh" "${BUILD_FLAG[@]}" ) 2>&1 | tee "$build_log" | awk '/^real /{print $2}')"
  HOST_NOTES="profile=$BUILD_PROFILE; prep=kill-mister-ui"
  if [[ "$RENDER_SCALE" == "2" ]]; then
    HOST_NOTES="${HOST_NOTES}; render_scale=2; design=960x540; render=1920x1080; font=PressStart2P"
  else
    HOST_NOTES="${HOST_NOTES}; render_scale=1; design=960x540; render=960x540; fb_scale=2; font=PressStart2P"
  fi
  rm -f "$build_log"
  [[ -f "$BIN" ]] || { echo "Build failed: missing $BIN" >&2; exit 1; }
else
  HOST_NOTES="skip-build; profile=$BUILD_PROFILE; prep=kill-mister-ui"
  if [[ "$RENDER_SCALE" == "2" ]]; then
    HOST_NOTES="${HOST_NOTES}; render_scale=2; design=960x540; render=1920x1080; font=PressStart2P"
  else
    HOST_NOTES="${HOST_NOTES}; render_scale=1; design=960x540; render=960x540; fb_scale=2; font=PressStart2P"
  fi
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
