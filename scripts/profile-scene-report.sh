#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Run a profiled scene on the MiSTer and generate local TSV/trace/SVG/HTML reports.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
source "$HERE/scripts/mister-supervision-lib.sh"
OUT_DIR="$HERE/build/frame-profiles"

usage() {
  cat <<'EOF'
Usage: scripts/profile-scene-report.sh SCENE [SECS] [LABEL]

Example:
  scripts/profile-scene-report.sh video_playback 5 VIDEO-SMOKE

Outputs:
  build/frame-profiles/<label>-frames.tsv
  build/frame-profiles/<label>-trace.json
  build/frame-profiles/<label>-chart.svg
  build/frame-profiles/<label>-heatmap.svg
  build/frame-profiles/<label>-report.html
EOF
}

scene="${1:-}"
secs="${2:-5}"
label="${3:-${scene:-scene}-$(date -u +%Y%m%dT%H%M%SZ)}"
if [[ -z "$scene" || "$scene" == "-h" || "$scene" == "--help" ]]; then
  usage
  exit 0
fi

mkdir -p "$OUT_DIR"
remote_tsv="/tmp/${label}-frames.tsv"
remote_trace="/tmp/${label}-trace.json"
remote_log="/tmp/${label}-profile.log"
local_tsv="$OUT_DIR/${label}-frames.tsv"
local_trace="$OUT_DIR/${label}-trace.json"
local_chart="$OUT_DIR/${label}-chart.svg"
local_heatmap="$OUT_DIR/${label}-heatmap.svg"
local_report="$OUT_DIR/${label}-report.html"

echo "==> Profile scene=$scene secs=$secs label=$label"
mister_suspend_launcher
trap 'mister_restart_launcher >/dev/null 2>&1 || true' EXIT
"$MISTER" run "kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true; sleep 5; MISTER_PROFILE=summary MISTER_PROFILE_FILE=$remote_tsv MISTER_TRACE_FILE=$remote_trace /media/fat/mister-magik-dev/mister-magik-fb ui $scene $secs >$remote_log 2>&1; grep -E 'frame_profile:|present-bandwidth' $remote_log"

echo "==> Pull profile artifacts"
"$MISTER" get "$remote_tsv" "$local_tsv"
"$MISTER" get "$remote_trace" "$local_trace"

echo "==> Generate reports"
"$HERE/scripts/frame-profile-chart.py" "$local_tsv" "$local_chart" --title "$label"
"$HERE/scripts/frame-profile-heatmap.py" "$local_tsv" "$local_heatmap" --title "$label dirty heatmap"
"$HERE/scripts/frame-profile-report.py" "$local_tsv" "$local_report" --title "$label" --trace "$local_trace"
"$HERE/scripts/frame-profile-histogram.py" "$local_tsv" --phase wall_us --phase slint_render_us --phase fb_present_us

echo "==> Wrote:"
printf '  %s\n' "$local_tsv" "$local_trace" "$local_chart" "$local_heatmap" "$local_report"
