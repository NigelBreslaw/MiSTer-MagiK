#!/usr/bin/env bash
# Run the real arcade screen through every raw screenshot transition effect.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  cat <<'EOF'
Usage: scripts/profile-preview-transition-mega.sh [LABEL] [--skip-build|--deploy-fast|--deploy-device] [--segment-secs N] [--transition-ms N] [--fb-format 8888|565] [--preview-format png|derived-png|raw-rgb|raw-rgb565]

Runs the real `ui arcade` surface with `MISTER_PREVIEW_TRANSITION=mega`.
Each effect gets --segment-secs seconds of held-scroll, then the trace is
summarized overall and by transition effect.
EOF
}

label="preview-transition-mega-$(date -u +%Y%m%dT%H%M%SZ)"
deploy="--skip-build"
segment_secs="5"
transition_ms="320"
fb_format="565"
preview_format="raw-rgb565"
visual_captures="0"
positionals=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) deploy="--skip-build"; shift ;;
    --deploy-fast) deploy="--deploy-fast"; shift ;;
    --deploy-device) deploy="--deploy-device"; shift ;;
    --segment-secs) segment_secs="${2:-}"; shift 2 ;;
    --transition-ms) transition_ms="${2:-}"; shift 2 ;;
    --fb-format) fb_format="${2:-}"; shift 2 ;;
    --preview-format) preview_format="${2:-}"; shift 2 ;;
    --visual-captures) visual_captures="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) positionals+=("$1"); shift ;;
  esac
done

if [[ "${#positionals[@]}" -ge 1 ]]; then label="${positionals[0]}"; fi
if [[ "${#positionals[@]}" -gt 1 ]]; then usage >&2; exit 2; fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then echo "label must contain only letters, numbers, _, ., or -" >&2; exit 2; fi
if [[ ! "$segment_secs" =~ ^[0-9]+$ || "$segment_secs" -lt 1 ]]; then echo "--segment-secs must be a positive integer" >&2; exit 2; fi
if [[ ! "$transition_ms" =~ ^[0-9]+$ || "$transition_ms" -lt 1 ]]; then echo "--transition-ms must be a positive integer" >&2; exit 2; fi

effect_count=10
secs=$((segment_secs * effect_count))

"$HERE/scripts/profile-preview-scroll.sh" \
  "$secs" held-scroll "$label" \
  "$deploy" \
  --fb-format "$fb_format" \
  --preview-blitter raw \
  --preview-format "$preview_format" \
  --transition mega \
  --transition-segment-secs "$segment_secs" \
  --transition-ms "$transition_ms" \
  --visual-captures "$visual_captures"
