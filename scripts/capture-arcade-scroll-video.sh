#!/usr/bin/env bash
# Record the real Arcade velocity-scroll scenario with the host USB capture device.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$HERE/build/arcade-scroll-captures"

label="arcade-scroll-video-$(date -u +%Y%m%dT%H%M%SZ)"
secs=10
capture_secs=20
device_index=0
device_name=""
size="1920x1080"
fps="60.000240"
ui_fb_size="${MISTER_UI_FB_SIZE:-auto}"

usage() {
  cat <<'EOF'
Usage: scripts/capture-arcade-scroll-video.sh [LABEL] [--secs N] [--capture-secs N] [--device-index N|--device-name NAME] [--size WxH] [--fps N] [--ui-fb-size auto|960x540|1280x720]

Starts a native macOS AVFoundation camera recording, then runs the real
Main-supervised Arcade velocity-scroll profile. The recording intentionally
starts before the launcher restart so the timed scroll window is captured.
Writes a probe file with the encoded video geometry and frame rate.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs) secs="${2:?}"; shift 2 ;;
    --capture-secs) capture_secs="${2:?}"; shift 2 ;;
    --device-index) device_index="${2:?}"; shift 2 ;;
    --device-name) device_name="${2:?}"; shift 2 ;;
    --size) size="${2:?}"; shift 2 ;;
    --fps) fps="${2:?}"; shift 2 ;;
    --ui-fb-size) ui_fb_size="${2:?}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *)
      label="$1"
      shift
      ;;
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
case "$ui_fb_size" in
  auto|960x540|1280x720) ;;
  *) echo "--ui-fb-size must be auto, 960x540, or 1280x720" >&2; exit 2 ;;
esac
if ! command -v ffprobe >/dev/null 2>&1; then
  echo "ffprobe is required to verify the captured video" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
video="$OUT_DIR/${label}.mov"
camera_log="$OUT_DIR/${label}.camera.log"
profile_log="$OUT_DIR/${label}.profile.log"
probe_log="$OUT_DIR/${label}.probe.txt"

echo "==> recording $video"
echo "==> requested capture ${size}@${fps}; ui_fb_size=$ui_fb_size"
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
}
trap cleanup EXIT

sleep 1
set +e
"$HERE/scripts/profile-arcade-scroll.sh" "$label" --secs "$secs" --scenario velocity-scroll --skip-build --ui-fb-size "$ui_fb_size" | tee "$profile_log"
profile_status=${PIPESTATUS[0]}
set -e
wait "$camera_pid"
trap - EXIT

{
  printf 'requested_size=%s\n' "$size"
  printf 'requested_fps=%s\n' "$fps"
  printf 'ui_fb_size=%s\n' "$ui_fb_size"
  ffprobe -hide_banner -v error \
    -select_streams v:0 \
    -show_entries stream=width,height,r_frame_rate,avg_frame_rate,nb_frames,duration \
    -of default=noprint_wrappers=1 "$video"
} >"$probe_log"

echo "wrote $video"
echo "wrote $camera_log"
echo "wrote $profile_log"
echo "wrote $probe_log"
if [[ "$profile_status" -ne 0 ]]; then
  echo "profile exited $profile_status after triggering the visual scroll; video was still finalized"
fi
