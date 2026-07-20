#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Capture framebuffer and HDMI evidence for the Menu FPGA scaler seam.
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
effect_profile_setup "scaler-seam" "unused.tsv"

label="scaler-seam-$(date -u +%Y%m%dT%H%M%SZ)"
camera_name="USB Video"
camera_secs=2
patterns="pixel-grid,vertical,horizontal,column-codes"
sizes="960x540,1280x720"
guards="0,1,2"

usage() {
  cat <<'EOF'
Usage: scripts/experiments/effects/capture-scaler-seam-matrix.sh [LABEL] [--patterns LIST] [--sizes LIST] [--guards LIST] [--camera-name NAME] [--camera-secs N]

Runs the already-deployed experiment binary. It never builds or deploys an RBF.
Each case retains a latch framebuffer capture, HDMI video and stills, active
latch status, launcher status, and scene log under build/scaler-seam/LABEL/.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --patterns) patterns="${2:?}"; shift 2 ;;
    --sizes) sizes="${2:?}"; shift 2 ;;
    --guards) guards="${2:?}"; shift 2 ;;
    --camera-name) camera_name="${2:?}"; shift 2 ;;
    --camera-secs) camera_secs="${2:?}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) label="$1"; shift ;;
  esac
done

effect_validate_label "$label"
effect_validate_positive_int "$camera_secs" "--camera-secs"
case "$patterns" in
  *[!A-Za-z0-9,_-]*) echo "invalid --patterns" >&2; exit 2 ;;
esac
case "$sizes" in
  *[!0-9,x]*) echo "invalid --sizes" >&2; exit 2 ;;
esac
case "$guards" in
  *[!0-9,]*) echo "invalid --guards" >&2; exit 2 ;;
esac

command -v ffmpeg >/dev/null 2>&1 || { echo "ffmpeg is required" >&2; exit 1; }
"$HERE/scripts/host-camera-native" list | grep -Fq "] $camera_name " || {
  echo "camera not found: $camera_name" >&2
  exit 1
}

root="$OUT_DIR/$label"
mkdir -p "$root"
effect_suspend_launcher

scene_pid=""
cleanup() {
  if [[ -n "$scene_pid" ]]; then
    "$MISTER" run "kill -9 '$scene_pid' 2>/dev/null || true" >/dev/null 2>&1 || true
  fi
  effect_cleanup_temp_files
}
trap cleanup EXIT INT TERM

IFS=',' read -r -a pattern_list <<<"$patterns"
IFS=',' read -r -a size_list <<<"$sizes"
IFS=',' read -r -a guard_list <<<"$guards"

for size in "${size_list[@]}"; do
  for guard in "${guard_list[@]}"; do
    for pattern in "${pattern_list[@]}"; do
      case_id="${size}-guard${guard}-${pattern}"
      case_dir="$root/$case_id"
      mkdir -p "$case_dir"
      echo "==> $case_id"

      scene_pid="$($MISTER run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
rm -f /tmp/scaler-seam.log
MISTER_SCREENSAVER='pixel-grid' MISTER_UI_FB_SIZE='$size' MISTER_FB_RIGHT_GUARD_COLS='$guard' MISTER_SCALER_PATTERN='$pattern' \
  '$REMOTE' ui screensaver 0 >/tmp/scaler-seam.log 2>&1 &
echo \$!
")"
      sleep 2

      "$MISTER" agent framebuffer-capture "$case_dir/framebuffer.png" \
        --json "$case_dir/framebuffer.json" >/dev/null
      "$MISTER" run "'$REMOTE' fpga-latch-report" >"$case_dir/latch-report.txt"
      "$MISTER" agent magik status --json >"$case_dir/magik-status.json"
      "$MISTER" get /tmp/scaler-seam.log "$case_dir/scene.log" >/dev/null

      "$HERE/scripts/host-camera-native" video --device-name "$camera_name" \
        --size 1920x1080 --fps 60 --duration "$camera_secs" \
        --output "$case_dir/hdmi.mov" >"$case_dir/camera.log" 2>&1
      if [[ ! -s "$case_dir/hdmi.mov" ]]; then
        echo "camera returned without a video for $case_id" >&2
        exit 1
      fi
      ffmpeg -hide_banner -loglevel error -y -i "$case_dir/hdmi.mov" \
        -vf "fps=3" -frames:v 3 "$case_dir/hdmi-%02d.png"
      for still in "$case_dir"/hdmi-*.png; do
        [[ -f "$still" ]] || continue
        stem="${still%.png}"
        ffmpeg -hide_banner -loglevel error -y -i "$still" \
          -vf "crop=640:540:640:270" -frames:v 1 -update 1 "${stem}-center.png"
        ffmpeg -hide_banner -loglevel error -y -i "$still" \
          -vf "crop=160:540:1760:270" -frames:v 1 -update 1 "${stem}-right.png"
      done

      "$MISTER" run "kill -9 '$scene_pid' 2>/dev/null || true" >/dev/null
      scene_pid=""
    done
  done
done

trap - EXIT INT TERM
cleanup
echo "wrote $root"
