#!/usr/bin/env bash
# Record the HDMI capture device while the Slint tear-pattern scene runs.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="$HERE/build/tear-pattern-captures"

label="tear-pattern-$(date -u +%Y%m%dT%H%M%SZ)"
secs=10
capture_secs=14
device_index=0
device_name=""
size="1920x1080"
fps="25"

usage() {
  cat <<'EOF'
Usage: scripts/capture-tear-pattern-video.sh [LABEL] [--secs N] [--capture-secs N] [--device-index N|--device-name NAME] [--size WxH] [--fps N]

Starts the native macOS USB-camera recorder, runs the deployed MiSTer MagiK
tear_pattern scene, writes a contact strip PNG, and probes the encoded video.
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

if ! command -v ffmpeg >/dev/null 2>&1; then
  echo "ffmpeg is required to build the contact strip" >&2
  exit 1
fi
if ! command -v ffprobe >/dev/null 2>&1; then
  echo "ffprobe is required to verify the captured video" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
video="$OUT_DIR/${label}.mov"
strip="$OUT_DIR/${label}.strip.png"
camera_log="$OUT_DIR/${label}.camera.log"
scene_log="$OUT_DIR/${label}.scene.log"
probe_log="$OUT_DIR/${label}.probe.txt"

echo "==> recording $video"
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
"$HERE/scripts/mister" run "kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true; sleep 0.5; /media/fat/mister-magik/mister-magik-fb ui tear_pattern $secs" | tee "$scene_log"
scene_status=${PIPESTATUS[0]}
set -e
wait "$camera_pid"
trap - EXIT

ffmpeg -hide_banner -loglevel warning -y \
  -i "$video" \
  -vf "fps=1,scale=480:-1,tile=5x2:padding=8:margin=8:color=black" \
  -frames:v 1 -update 1 "$strip"

ffprobe -hide_banner -v error \
  -select_streams v:0 \
  -show_entries stream=width,height,r_frame_rate,avg_frame_rate,nb_frames,duration \
  -of default=noprint_wrappers=1 "$video" >"$probe_log"

echo "wrote $video"
echo "wrote $strip"
echo "wrote $camera_log"
echo "wrote $scene_log"
echo "wrote $probe_log"
if [[ "$scene_status" -ne 0 ]]; then
  echo "tear_pattern scene exited $scene_status after capture; video was still finalized" >&2
  exit "$scene_status"
fi
