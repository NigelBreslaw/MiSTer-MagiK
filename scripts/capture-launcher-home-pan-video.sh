#!/usr/bin/env bash
# Record the real Home-row repeat-hold pan scenario with the host USB capture.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$HERE/build/launcher-home-pan-captures"
MISTER="$HERE/scripts/mister"
REMOTE="/media/fat/mister-magik/mister-magik-fb"
REMOTE_LOG="/tmp/mister-magik-home-pan-capture.log"
source "$HERE/scripts/mister-supervision-lib.sh"

label="launcher-home-pan-$(date -u +%Y%m%dT%H%M%SZ)"
secs=30
capture_secs=40
strip_start=10
device_index=0
device_name=""
size="1920x1080"
fps="25"
present_backend="${MISTER_PRESENT_BACKEND:-}"
ui_fb_size="${MISTER_UI_FB_SIZE:-auto}"
present_delay_us="${MISTER_FB_PRESENT_DELAY_US:-0}"

usage() {
  cat <<'EOF'
Usage: scripts/capture-launcher-home-pan-video.sh [LABEL] [--secs N] [--capture-secs N] [--strip-start N] [--device-index N|--device-name NAME] [--size WxH] [--fps N] [--present-backend fpga-vblank-latch-hidden] [--ui-fb-size auto|960x540|1280x720] [--present-delay-us N]

Starts the native macOS AVFoundation USB capture, then runs the real launcher
Home-row home-repeat-hold benchmark. That scenario holds left/right through the
normal input repeat path, including the initial delay and rapid repeat phase.
Writes the captured video plus full-frame and center-crop contact strips.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs) secs="${2:?}"; shift 2 ;;
    --capture-secs) capture_secs="${2:?}"; shift 2 ;;
    --strip-start) strip_start="${2:?}"; shift 2 ;;
    --device-index) device_index="${2:?}"; shift 2 ;;
    --device-name) device_name="${2:?}"; shift 2 ;;
    --size) size="${2:?}"; shift 2 ;;
    --fps) fps="${2:?}"; shift 2 ;;
    --present-backend) present_backend="${2:?}"; shift 2 ;;
    --ui-fb-size) ui_fb_size="${2:?}"; shift 2 ;;
    --present-delay-us) present_delay_us="${2:?}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) label="$1"; shift ;;
  esac
done

if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$secs" =~ ^[0-9]+$ || "$secs" -lt 1 ]]; then
  echo "secs must be a positive integer" >&2
  exit 2
fi
if [[ ! "$capture_secs" =~ ^[0-9]+$ || "$capture_secs" -lt "$secs" ]]; then
  echo "capture-secs must be an integer >= secs" >&2
  exit 2
fi
if [[ ! "$strip_start" =~ ^[0-9]+$ ]]; then
  echo "strip-start must be a non-negative integer" >&2
  exit 2
fi
case "$present_backend" in
  ""|fpga-vblank-latch-hidden) ;;
  *) echo "--present-backend must be fpga-vblank-latch-hidden when set" >&2; exit 2 ;;
esac
case "$ui_fb_size" in
  auto|960x540|1280x720) ;;
  *) echo "--ui-fb-size must be auto, 960x540, or 1280x720" >&2; exit 2 ;;
esac
if [[ ! "$present_delay_us" =~ ^[0-9]+$ ]]; then
  echo "--present-delay-us must be a non-negative integer" >&2
  exit 2
fi
if ! command -v ffmpeg >/dev/null 2>&1 || ! command -v ffprobe >/dev/null 2>&1; then
  echo "ffmpeg and ffprobe are required to finalize the capture strips" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
video="$OUT_DIR/${label}.mov"
camera_log="$OUT_DIR/${label}.camera.log"
profile_log="$OUT_DIR/${label}.profile.log"
remote_log="$OUT_DIR/${label}.remote.log"
probe_log="$OUT_DIR/${label}.probe.txt"
strip="$OUT_DIR/${label}.strip.png"
tear_strip="$OUT_DIR/${label}.tear-strip.png"

echo "==> recording $video"
echo "==> requested capture ${size}@${fps}; backend=${present_backend:-fb0-dirty}; ui_fb_size=$ui_fb_size present_delay_us=$present_delay_us"
camera_selector=(--device-index "$device_index")
if [[ -n "$device_name" ]]; then
  camera_selector=(--device-name "$device_name")
fi
env CLANG_MODULE_CACHE_PATH="${CLANG_MODULE_CACHE_PATH:-/tmp/swift-module-cache}" \
  "$HERE/scripts/host-camera-native" video \
  "${camera_selector[@]}" \
  --size "$size" \
  --fps "$fps" \
  --duration "$capture_secs" \
  --output "$video" >"$camera_log" 2>&1 &
camera_pid=$!

cleanup() {
  if kill -0 "$camera_pid" >/dev/null 2>&1; then
    kill "$camera_pid" >/dev/null 2>&1 || true
    wait "$camera_pid" >/dev/null 2>&1 || true
  fi
  mister_restart_launcher >/dev/null 2>&1 || true
}
trap cleanup EXIT

sleep 1
set +e
present_env=""
if [[ -n "$present_backend" ]]; then
  present_env="MISTER_PRESENT_BACKEND='$present_backend'"
fi
mister_suspend_launcher
"$MISTER" run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
test -x '$REMOTE' || chmod +x '$REMOTE'
MISTER_LAUNCHER_BENCH_SCENARIO=home-repeat-hold MISTER_UI_FB_SIZE='$ui_fb_size' MISTER_FB_PRESENT_DELAY_US='$present_delay_us' $present_env '$REMOTE' ui launcher '$secs' >'$REMOTE_LOG' 2>&1
status=\$?
sed -n '1,220p' '$REMOTE_LOG' 2>/dev/null || true
exit \$status
" | tee "$profile_log"
profile_status=${PIPESTATUS[0]}
set -e
wait "$camera_pid"
trap - EXIT
mister_restart_launcher >/dev/null 2>&1 || true
"$MISTER" get "$REMOTE_LOG" "$remote_log" >/dev/null 2>&1 || true

{
  printf 'requested_size=%s\n' "$size"
  printf 'requested_fps=%s\n' "$fps"
  printf 'present_backend=%s\n' "${present_backend:-fb0-dirty}"
  printf 'ui_fb_size=%s\n' "$ui_fb_size"
  printf 'present_delay_us=%s\n' "$present_delay_us"
  ffprobe -hide_banner -v error \
    -select_streams v:0 \
    -show_entries stream=width,height,r_frame_rate,avg_frame_rate,nb_frames,duration \
    -of default=noprint_wrappers=1 "$video"
} >"$probe_log"

ffmpeg -hide_banner -loglevel error -y -ss "$strip_start" -i "$video" \
  -vf "fps=5,scale=480:-1,tile=5x4" -frames:v 1 "$strip"
ffmpeg -hide_banner -loglevel error -y -ss "$strip_start" -i "$video" \
  -vf "fps=25,crop=iw:ih/3:0:ih/3,scale=480:-1,tile=5x5" -frames:v 1 "$tear_strip"

echo "wrote $video"
echo "wrote $camera_log"
echo "wrote $profile_log"
echo "wrote $remote_log"
echo "wrote $probe_log"
echo "wrote $strip"
echo "wrote $tear_strip"
if [[ "$profile_status" -ne 0 ]]; then
  echo "profile exited $profile_status after triggering the visual pan; video was still finalized"
fi
