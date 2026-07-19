#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Experimental: run the real launcher Arcade screen through every raw screenshot transition effect.
set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
HERE="$(experiment_repo_root)"
MISTER="$HERE/scripts/mister"
REMOTE="/media/fat/mister-magik-dev/mister-magik-fb"
REMOTE_ENV="/media/fat/mister-magik-dev/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"
OUT_DIR="$HERE/build/preview-scroll-profiles"

usage() {
  cat <<'EOF'
Usage: scripts/experiments/preview/profile-preview-transition-mega.sh [LABEL] [--skip-build|--deploy-device] [--segment-secs N] [--transition-ms N] [--preview-format raw-rgb565] [--visual-captures 0]

Runs an experiment-enabled Main-supervised launcher Arcade screen with
`MISTER_PREVIEW_TRANSITION=mega`.
Each effect gets --segment-secs seconds of held-scroll, then the trace is
written under build/preview-scroll-profiles.
Requires a deployed bench-tools+experiments MagiK binary; --deploy-device builds one.
EOF
}

label="preview-transition-mega-$(date -u +%Y%m%dT%H%M%SZ)"
deploy="--skip-build"
segment_secs="5"
transition_ms="320"
preview_format="raw-rgb565"
visual_captures="0"
positionals=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) deploy="--skip-build"; shift ;;
    --deploy-device) deploy="--deploy-device"; shift ;;
    --segment-secs) segment_secs="${2:-}"; shift 2 ;;
    --transition-ms) transition_ms="${2:-}"; shift 2 ;;
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
case "$preview_format" in raw-rgb565|raw565|rgb565|565) ;; *) echo "--preview-format must be raw-rgb565" >&2; exit 2 ;; esac
if [[ ! "$visual_captures" =~ ^[0-9]+$ ]]; then echo "--visual-captures must be an integer" >&2; exit 2; fi

if [[ "$deploy" == "--deploy-device" ]]; then
  "$HERE/scripts/deploy-rust.sh" --device --experiments --bench-tools
fi

require_preview_mega_transitions "$MISTER" "$REMOTE"
effect_count="$("$MISTER" run "'$REMOTE' preview-transitions" | (grep -E '^[a-z0-9-]+$' || true) | wc -l | tr -d ' ')"
if [[ ! "$effect_count" =~ ^[0-9]+$ || "$effect_count" -lt 1 ]]; then
  echo "failed to count deployed preview transition labels" >&2
  exit 1
fi
secs=$((segment_secs * effect_count))

mkdir -p "$OUT_DIR"
env_file="$(mktemp)"
remote_tsv="/tmp/${label}-mega.tsv"
local_tsv="$OUT_DIR/${label}-mega.tsv"
local_log="$OUT_DIR/${label}-mega.log"

cleanup() {
  rm -f "$env_file"
  "$MISTER" run "rm -f '$REMOTE_ENV'" >/dev/null 2>&1 || true
  "$MISTER" agent magik restart-launcher >/dev/null 2>&1 || true
}
trap cleanup EXIT

{
  printf 'export MISTER_CATALOG_REFRESH=off\n'
  printf 'export MISTER_LAUNCHER_START_SCREEN=arcade\n'
  printf 'export MISTER_LAUNCHER_LOCK_SCREEN=arcade\n'
  printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=held-scroll\n'
  printf 'export MISTER_PREVIEW_TRACE=1\n'
  printf 'export MISTER_PREVIEW_SCROLL_TRACE_SECS=%q\n' "$secs"
  printf 'export MISTER_PREVIEW_SCROLL_TRACE=%q\n' "$remote_tsv"
  printf 'export MISTER_PREVIEW_FORMAT=%q\n' "$preview_format"
  printf 'export MISTER_PREVIEW_TRANSITION=mega\n'
  printf 'export MISTER_PREVIEW_TRANSITION_SEGMENT_SECS=%q\n' "$segment_secs"
  printf 'export MISTER_PREVIEW_TRANSITION_MS=%q\n' "$transition_ms"
} >"$env_file"

echo "==> preview transition mega label=$label effects=$effect_count secs=$secs segment_secs=$segment_secs transition_ms=$transition_ms"
"$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
"$MISTER" run "rm -f '$REMOTE_LOG' '$remote_tsv'" >/dev/null
"$MISTER" agent magik restart-launcher >/dev/null
sleep $((secs + 7))
"$MISTER" get "$remote_tsv" "$local_tsv" >/dev/null
"$MISTER" get "$REMOTE_LOG" "$local_log" >/dev/null || true
echo "wrote $local_tsv"
echo "wrote $local_log"

if [[ "$visual_captures" != "0" ]]; then
  echo "visual captures are handled by the release preview-scroll benchmark; rerun that script for production visuals" >&2
fi
