#!/usr/bin/env bash
# Benchmark native retro framebuffer effects on the MiSTer.
#
#   scripts/bench-effects.sh EFFECTS-20260607 --device --replace-label
#
# Appends one row per effect/mode/size to history/toolchain-bench/results-effects.tsv.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$HERE/magik-gui"
REMOTE="/media/fat/mister-magik/mister-magik-fb"
BENCH_DIR="$HERE/history/toolchain-bench"
TSV="$BENCH_DIR/results-effects.tsv"
MISTER="$HERE/scripts/mister"

LABEL="EFFECTS"
BUILD_PROFILE=release
BUILD_FLAG=()
SKIP_BUILD=0
SKIP_DEVICE=0
REPLACE_LABEL=0
SCENE_SECS=10
MATRIX=default
EFFECT_FILTER=all
MODE_FILTER=both
SIZE_FILTER=480x270
FILL_FILTER=half
SETTLE_SECS="${MISTER_BENCH_SETTLE_SECS:-5}"

EFFECTS=(
  palette_cycle plasma copper_bars starfield crt_pass tile_parallax
  mode7_floor afterimage dither_spotlight wipe_transition chunky_distortion fire_haze
  vhs_glitch
)
SIZES=(320x180 320x224 480x270 640x360 960x540)

usage() {
  sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'
  echo ""
  echo "Options: --device  --skip-build  --skip-device  --replace-label"
  echo "         --scene-secs N  --effect NAME|all  --mode raw|overlay|both"
  echo "         --size WIDTHxHEIGHT  --fill native|half|full  --matrix default|scale-sweep|full  -h"
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage 0 ;;
    --device) BUILD_PROFILE=release-device; BUILD_FLAG+=(--device); shift ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --skip-device) SKIP_DEVICE=1; shift ;;
    --replace-label) REPLACE_LABEL=1; shift ;;
    --scene-secs|--ui-secs) SCENE_SECS="${2:?}"; shift 2 ;;
    --effect) EFFECT_FILTER="${2:?}"; shift 2 ;;
    --mode) MODE_FILTER="${2:?}"; shift 2 ;;
    --size) SIZE_FILTER="${2:?}"; shift 2 ;;
    --fill) FILL_FILTER="${2:?}"; shift 2 ;;
    --matrix) MATRIX="${2:?}"; shift 2 ;;
    -*) echo "Unknown option: $1" >&2; usage 1 ;;
    *) LABEL="$1"; shift ;;
  esac
done

mkdir -p "$BENCH_DIR"

LOCK_DIR="${TMPDIR:-/tmp}/mister-magik-bench-effects.lock"
if ! mkdir "$LOCK_DIR" 2>/dev/null; then
  echo "ERROR: another bench-effects.sh run is already active (lock: $LOCK_DIR)" >&2
  echo "       Stop the old run or remove the lock if it is stale." >&2
  exit 1
fi
trap 'rmdir "$LOCK_DIR" 2>/dev/null || true' EXIT INT TERM

HEADER="label	effect	mode	fill	internal	scale	date	rustc	compile_sec	bytes	frames	fps	effect_us	slint_us	scale_copy_us	vsync_us	wall_us	cpu_mean	cpu_max	rss_kb	visual_ok	notes"
if [[ ! -f "$TSV" ]] || ! head -1 "$TSV" | grep -q $'^label\teffect'; then
  echo "$HEADER" >"$TSV"
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

effects_for_run() {
  if [[ "$EFFECT_FILTER" == "all" ]]; then
    printf '%s\n' "${EFFECTS[@]}"
  else
    printf '%s\n' "$EFFECT_FILTER"
  fi
}

modes_for_run() {
  case "$MODE_FILTER" in
    raw) echo raw ;;
    overlay) echo overlay ;;
    both) printf '%s\n' raw overlay ;;
    *) echo "Unknown mode: $MODE_FILTER" >&2; exit 2 ;;
  esac
}

sizes_for_run() {
  case "$MATRIX" in
    default) echo "$SIZE_FILTER" ;;
    scale-sweep|full) printf '%s\n' "${SIZES[@]}" ;;
    *) echo "Unknown matrix: $MATRIX" >&2; exit 2 ;;
  esac
}

append_row() {
  local effect="$1" mode="$2" fill="$3" internal="$4" scale="$5" frames="$6" fps="$7"
  local effect_us="$8" slint_us="$9" scale_copy_us="${10}" vsync_us="${11}" wall_us="${12}"
  local cpu_mean="${13}" cpu_max="${14}" rss_kb="${15}" visual_ok="${16}" notes="${17}"
  notes="${notes//	/ }"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$LABEL" "$effect" "$mode" "$fill" "$internal" "$scale" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    "$(rustc_version)" "${HOST_COMPILE_SEC:-}" "${HOST_BYTES:-}" "$frames" "$fps" \
    "$effect_us" "$slint_us" "$scale_copy_us" "$vsync_us" "$wall_us" \
    "$cpu_mean" "$cpu_max" "$rss_kb" "$visual_ok" "$notes" >>"$TSV"
}

run_one() {
  local effect="$1" mode="$2" size="$3"
  local ui_log ui_full capture_at raw png
  ui_log="$(mktemp)"
  ui_full="$(mktemp)"
  capture_at=$((SCENE_SECS > 4 ? SCENE_SECS - 2 : 2))

  mister run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
kill -9 \$(pidof MiSTer_MagiK) 2>/dev/null || true
kill -9 \$(pidof MiSTer) 2>/dev/null || true
sleep $SETTLE_SECS
MISTER_EFFECT_BENCH_LABEL=$LABEL $REMOTE effect-bench $effect $SCENE_SECS $mode $size $FILL_FILTER > /tmp/effect-bench-ui.log 2>&1 &
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
while [ \$i -lt $SCENE_SECS ]; do
  if kill -0 \$UI_PID 2>/dev/null; then
    if [ \$FB_CAPTURED -eq 0 ] && [ \$i -ge $capture_at ]; then
      dd if=/dev/fb0 of=/tmp/effect-bench-fb.raw bs=1M count=32 2>/dev/null && FB_CAPTURED=1
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
echo ___BENCH_CPU_MEAN___
if [ \$CPU_N -gt 0 ]; then echo \$((CPU_SUM / CPU_N)); else echo 0; fi
echo ___BENCH_CPU_MAX___
echo \$CPU_MAX
echo ___BENCH_RSS___
echo \$RSS
echo ___BENCH_UI_RC___
echo \$UI_RC
echo ___BENCH_UI_LOG___
cat /tmp/effect-bench-ui.log
" >"$ui_full" 2>&1 || true

  if grep -q ___BENCH_UI_LOG___ "$ui_full"; then
    sed -n '/___BENCH_UI_LOG___/,$p' "$ui_full" | tail -n +2 >"$ui_log"
  else
    cp "$ui_full" "$ui_log"
  fi
  cp "$ui_log" "$BENCH_DIR/${LABEL}-${effect}-${mode}-${size}-ui.log" 2>/dev/null || true

  local result cpu_mean cpu_max rss_kb ui_rc fb_captured visual_ok notes
  result="$(awk -F '\t' '$1 == "effect_bench_result" {print; exit}' "$ui_log")"
  cpu_mean="$(sed -n '/___BENCH_CPU_MEAN___/{n;p;}' "$ui_full" 2>/dev/null | head -1)"
  cpu_max="$(sed -n '/___BENCH_CPU_MAX___/{n;p;}' "$ui_full" 2>/dev/null | head -1)"
  rss_kb="$(sed -n '/___BENCH_RSS___/{n;p;}' "$ui_full" 2>/dev/null | head -1)"
  ui_rc="$(sed -n '/___BENCH_UI_RC___/{n;p;}' "$ui_full" 2>/dev/null | head -1)"
  fb_captured="$(sed -n '/___BENCH_FB_CAPTURED___/{n;p;}' "$ui_full" 2>/dev/null | head -1)"
  if [[ -n "$rss_kb" && "$rss_kb" =~ ^[0-9]+$ && "$rss_kb" -lt 100000 ]]; then
    rss_kb=$((rss_kb * 4))
  fi

  visual_ok=yes
  notes="${HOST_NOTES:-}"
  if [[ "${ui_rc:-1}" != "0" ]] && ! grep -q '^effect_bench_result' "$ui_log"; then
    visual_ok=no
    notes="${notes:+$notes; }ui-rc=${ui_rc:-?}"
  fi

  raw="$HERE/build/effect-bench-fb.raw"
  png="$BENCH_DIR/${LABEL}-${effect}-${mode}-${size}-fb.png"
  mkdir -p "$HERE/build"
  if [[ "$fb_captured" == "1" ]] && mister get /tmp/effect-bench-fb.raw "$raw" >/dev/null 2>&1 \
    && mister raw-to-png "$raw" 1920 1080 "$png" >/dev/null 2>&1; then
    :
  else
    visual_ok=no
    notes="${notes:+$notes; }capture-fail"
  fi

  if [[ -n "$result" ]]; then
    local _tag result_label result_effect result_mode result_fill internal scale frames fps effect_us slint_us scale_copy_us vsync_us wall_us
    IFS=$'\t' read -r _tag result_label result_effect result_mode result_fill internal scale frames fps effect_us slint_us scale_copy_us vsync_us wall_us <<<"$result"
    append_row "$result_effect" "$result_mode" "$result_fill" "$internal" "$scale" "$frames" "$fps" \
      "$effect_us" "$slint_us" "$scale_copy_us" "$vsync_us" "$wall_us" \
      "${cpu_mean:-}" "${cpu_max:-}" "${rss_kb:-}" "$visual_ok" "$notes"
    echo "    [$effect/$mode/$size] fps=$fps effect=${effect_us}us slint=${slint_us}us copy=${scale_copy_us}us cpu=${cpu_mean:-?}%"
  else
    append_row "$effect" "$mode" "$FILL_FILTER" "$size" "" "" "" "" "" "" "" "" \
      "${cpu_mean:-}" "${cpu_max:-}" "${rss_kb:-}" no "${notes:+$notes; }no-result-line"
    echo "    [$effect/$mode/$size] no result line" >&2
  fi

  rm -f "$ui_log" "$ui_full"
}

echo "==> Effects bench label=$LABEL matrix=$MATRIX fill=$FILL_FILTER secs=$SCENE_SECS"

HOST_COMPILE_SEC=""
HOST_BYTES=""
HOST_NOTES="profile=$BUILD_PROFILE; prep=kill-mister-ui"
BIN="$RUST_DIR/target/armv7-unknown-linux-gnueabihf/$BUILD_PROFILE/mister-magik-fb"

if [[ "$SKIP_BUILD" -eq 0 ]]; then
  echo "==> Cross-build (timed)"
  build_log="$(mktemp)"
  HOST_COMPILE_SEC="$( ( time -p "$RUST_DIR/build-arm.sh" "${BUILD_FLAG[@]}" ) 2>&1 | tee "$build_log" | awk '/^real /{print $2}')"
  rm -f "$build_log"
fi

[[ -f "$BIN" ]] || { echo "No binary at $BIN" >&2; exit 1; }
if stat -f%z "$BIN" >/dev/null 2>&1; then
  HOST_BYTES="$(stat -f%z "$BIN")"
else
  HOST_BYTES="$(stat -c%s "$BIN")"
fi

if [[ "$SKIP_DEVICE" -eq 0 ]]; then
  echo "==> Deploy $BIN"
  mister run "kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true; mkdir -p /media/fat/mister-magik"
  mister put "$BIN" "$REMOTE"
  mister run "chmod +x $REMOTE"

  for size in $(sizes_for_run); do
    if [[ "$MATRIX" == "scale-sweep" ]]; then
      modes=(raw)
    else
      mapfile -t modes < <(modes_for_run)
    fi
    mapfile -t effects < <(effects_for_run)
    for effect in "${effects[@]}"; do
      for mode in "${modes[@]}"; do
        echo "==> Effect $effect mode=$mode size=$size"
        run_one "$effect" "$mode" "$size"
      done
    done
  done
else
  for size in $(sizes_for_run); do
    mapfile -t effects < <(effects_for_run)
    mapfile -t modes < <(modes_for_run)
    for effect in "${effects[@]}"; do
      for mode in "${modes[@]}"; do
        append_row "$effect" "$mode" "$FILL_FILTER" "$size" "" "" "" "" "" "" "" "" "" "" "" "n/a" "${HOST_NOTES:+$HOST_NOTES; }skip-device"
      done
    done
  done
fi

echo "==> Results: $TSV"
column -t -s $'\t' "$TSV" 2>/dev/null || cat "$TSV"
