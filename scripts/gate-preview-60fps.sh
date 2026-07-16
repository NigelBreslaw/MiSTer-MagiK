#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="$ROOT/scripts/profile-preview-scroll.sh"
PRESENT_TRACE="$ROOT/scripts/bench/analyze/launcher-present-trace.py"
OUT_DIR="$ROOT/build/preview-scroll-profiles"

secs=60
deploy_arg="--skip-build"
visual_captures=0
p99_work_us=14500
self_test=0
baseline_label=""
label=""

usage() {
  cat <<'EOF'
Usage: scripts/gate-preview-60fps.sh LABEL [--secs N] [--skip-build|--deploy-device] [--visual-captures N] [--p99-work-us N] [--baseline-label BASE] [--self-test]

Runs the final Arcade preview fade pacing gate:
  - held-scroll fade
  - turbo-hold fade

Fails if either run has non-exact screenshot previews, vsync
fallback/timeout/error, non-zero max vsync miss streak, or p99 work above the
threshold. Reports work-over-budget outliers separately so scheduler spikes do
not hide p99 headroom.

When --baseline-label is provided, also compares BASE-FADE-VEL/TURBO traces
against the current run and fails on present-path regressions:
  - cached_present_us p95/p99 > +5%
  - fb_present_us p95/p99 > +5%
  - rows p95/p99 increase > 1 row
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs) secs="${2:-}"; shift 2 ;;
    --skip-build) deploy_arg="--skip-build"; shift ;;
    --deploy-device) deploy_arg="--deploy-device"; shift ;;
    --visual-captures) visual_captures="${2:-}"; shift 2 ;;
    --p99-work-us) p99_work_us="${2:-}"; shift 2 ;;
    --baseline-label) baseline_label="${2:-}"; shift 2 ;;
    --self-test) self_test=1; shift ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) if [[ -z "$label" ]]; then label="$1"; else echo "unexpected argument: $1" >&2; exit 2; fi; shift ;;
  esac
done

gate_trace() {
  local name="$1" tsv="$2"
  local works
  works="$(mktemp "${TMPDIR:-/tmp}/preview-gate-work.XXXXXX")"
  local summary
  summary="$(
    awk -F '\t' -v works="$works" '
      NR == 1 {
        for (i = 1; i <= NF; i++) col[$i] = i
        required = "frame cache_state transition_effect transition_progress prepare_us slint_render_us custom_draw_us fb_present_us vsync_source vsync_miss_streak"
        split(required, req, " ")
        for (i in req) {
          if (!(req[i] in col)) {
            printf "missing-column:%s\n", req[i]
            exit 3
          }
        }
        next
      }
      NF && $(col["frame"]) + 0 > 30 {
        n++
        work = $(col["prepare_us"]) + $(col["slint_render_us"]) + $(col["custom_draw_us"]) + $(col["fb_present_us"])
        print work > works
        if (work > 16667) work_over++
        cache_state = $(col["cache_state"])
        if (cache_state == "exact") exact++
        else if (cache_state == "empty") empty++
        else {
          non_exact++
          if (non_exact <= 10) {
            printf "%s preview_exact_gate miss frame=%s cache_state=%s\n", name, $(col["frame"]), cache_state > "/dev/stderr"
          }
        }
        transition_effect = $(col["transition_effect"])
        transition_progress = $(col["transition_progress"]) + 0
        if (transition_effect == "fade" && transition_progress > 0 && transition_progress < 1) fade_rows++
        source = $(col["vsync_source"])
        if (source == "vsync") vsync++
        else if (source == "fallback") fallback++
        else if (source == "timeout") timeout++
        else if (source == "error") error++
        else other_source++
        miss = $(col["vsync_miss_streak"]) + 0
        if (miss > max_miss) max_miss = miss
      }
      END {
        printf "%d %d %d %d %d %d %d %d %d %d %d %d\n",
          n + 0, work_over + 0, vsync + 0, fallback + 0,
          timeout + 0, error + 0, other_source + 0, max_miss + 0,
          exact + 0, empty + 0, non_exact + 0, fade_rows + 0
      }
    ' "$tsv"
  )"
  read -r frames work_over vsync fallback timeout error other_source max_miss exact empty non_exact fade_rows <<<"$summary"
  if [[ "${frames:-0}" == missing-column:* ]]; then
    echo "validity_tsv	label=$name	valid=0	invalid_reason=missing_column	detail=${frames#missing-column:}"
    echo "$name gate failed: ${frames#missing-column:} column missing in $tsv" >&2
    rm -f "$works"
    return 9
  fi
  if [[ "$frames" -le 0 ]]; then
    echo "validity_tsv	label=$name	valid=0	invalid_reason=no_frames	detail=$tsv"
    echo "$name gate failed: no frames after frame 30 in $tsv" >&2
    rm -f "$works"
    return 9
  fi
  local p99_index p99_work
  p99_index=$((frames * 99 / 100))
  if [[ "$p99_index" -lt 1 ]]; then p99_index=1; fi
  p99_work="$(sort -n "$works" | awk -v idx="$p99_index" 'NR == idx { print; exit }')"
  rm -f "$works"

  echo "$name gate frames_after_30=$frames p99_work_us=$p99_work work_gt_16667=$work_over vsync=$vsync fallback=$fallback timeout=$timeout error=$error other_source=$other_source max_miss_streak=$max_miss exact=$exact empty=$empty non_exact=$non_exact fade_rows=$fade_rows"
  if [[ "$fallback" -ne 0 || "$timeout" -ne 0 || "$error" -ne 0 || "$other_source" -ne 0 || "$max_miss" -ne 0 || "${non_exact:-0}" -ne 0 || "${fade_rows:-0}" -lt 2 || "$p99_work" -ge "$p99_work_us" ]]; then
    echo "validity_tsv	label=$name	valid=0	invalid_reason=gate_failed	detail=p99_work_us=$p99_work threshold=$p99_work_us fallback=$fallback timeout=$timeout error=$error other_source=$other_source max_miss_streak=$max_miss exact=$exact empty=$empty non_exact=$non_exact fade_rows=$fade_rows"
    echo "$name gate failed" >&2
    return 9
  fi
  echo "validity_tsv	label=$name	valid=1	invalid_reason=ok	detail=p99_work_us=$p99_work threshold=$p99_work_us fallback=$fallback timeout=$timeout error=$error other_source=$other_source max_miss_streak=$max_miss exact=$exact empty=$empty non_exact=0 fade_rows=$fade_rows"
}

write_self_test_trace() {
  local path="$1" work="$2" source="$3" miss="$4"
  {
    echo $'frame\tcache_state\ttransition_effect\ttransition_progress\tprepare_us\tslint_render_us\tcustom_draw_us\tfb_present_us\tvsync_source\tvsync_miss_streak'
    for frame in $(seq 0 40); do
      progress="0.5"
      echo "${frame}"$'\texact\tfade\t'"${progress}"$'\t'"${work}"$'\t0\t0\t0\t'"${source}"$'\t'"${miss}"
    done
  } >"$path"
}

write_preview_miss_self_test_trace() {
  local path="$1"
  {
    echo $'frame\tcache_state\ttransition_effect\ttransition_progress\tprepare_us\tslint_render_us\tcustom_draw_us\tfb_present_us\tvsync_source\tvsync_miss_streak'
    for frame in $(seq 0 40); do
      state="exact"
      if [[ "$frame" == "35" ]]; then state="stale"; fi
      echo "${frame}"$'\t'"${state}"$'\tfade\t0.5\t1000\t0\t0\t0\tvsync\t0'
    done
  } >"$path"
}

write_present_self_test_trace() {
  local path="$1" cached_present="$2" fb_present="$3" rows="$4" source="$5" miss="$6"
  {
    echo $'frame\tcache_state\ttransition_effect\ttransition_progress\tarcade_update\trows\tprepare_us\tslint_render_us\tcustom_draw_us\tfb_present_us\tcached_present_us\tarcade_list_present_us\tvsync_source\tvsync_miss_streak'
    for frame in $(seq 0 180); do
      echo "${frame}"$'\texact\tfade\t0.5\tscroll:-12\t'"${rows}"$'\t0\t0\t0\t'"${fb_present}"$'\t'"${cached_present}"$'\t500\t'"${source}"$'\t'"${miss}"
    done
  } >"$path"
}

if [[ "$self_test" == "1" ]]; then
  tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/preview-gate-self.XXXXXX")"
  trap 'rm -rf "$tmpdir"' EXIT
  write_self_test_trace "$tmpdir/good.tsv" 1000 vsync 0
  gate_trace self-good "$tmpdir/good.tsv"
  write_self_test_trace "$tmpdir/bad-p99.tsv" 15000 vsync 0
  if gate_trace self-bad-p99 "$tmpdir/bad-p99.tsv" >/dev/null 2>&1; then
    echo "self-test expected p99 gate failure" >&2
    exit 1
  fi
  write_self_test_trace "$tmpdir/bad-vsync.tsv" 1000 fallback 1
  if gate_trace self-bad-vsync "$tmpdir/bad-vsync.tsv" >/dev/null 2>&1; then
    echo "self-test expected vsync gate failure" >&2
    exit 1
  fi
  write_preview_miss_self_test_trace "$tmpdir/bad-preview.tsv"
  if gate_trace self-bad-preview "$tmpdir/bad-preview.tsv" >/dev/null 2>&1; then
    echo "self-test expected non-exact preview gate failure" >&2
    exit 1
  fi
  write_self_test_trace "$tmpdir/bad-cut.tsv" 1000 vsync 0
  sed -i.bak $'s/\tfade\t0.5\t/\tfade\t1\t/g' "$tmpdir/bad-cut.tsv"
  if gate_trace self-bad-cut "$tmpdir/bad-cut.tsv" >/dev/null 2>&1; then
    echo "self-test expected missing fade gate failure" >&2
    exit 1
  fi
  write_present_self_test_trace "$tmpdir/present-before.tsv" 400 900 704 vsync 0
  write_present_self_test_trace "$tmpdir/present-after-bad.tsv" 500 1060 704 vsync 0
  gate_trace self-present-broad-still-ok "$tmpdir/present-after-bad.tsv" >/dev/null
  if "$PRESENT_TRACE" compare "$tmpdir/present-before.tsv" "$tmpdir/present-after-bad.tsv" --case self-present-regression >/dev/null 2>&1; then
    echo "self-test expected present-path comparison failure while broad gate passes" >&2
    exit 1
  fi
  echo "gate-preview-60fps self-test ok"
  exit 0
fi

if [[ -z "$label" ]]; then
  usage >&2
  exit 2
fi

run_and_gate() {
  local scenario="$1" suffix="$2"
  local run_label="${label}-${suffix}"
  "$PROFILE" "$secs" "$scenario" "$run_label" "$deploy_arg" --visual-captures "$visual_captures"
  gate_trace "$run_label" "$OUT_DIR/${run_label}-arcade.tsv"
  if [[ -n "$baseline_label" ]]; then
    local baseline_tsv="$OUT_DIR/${baseline_label}-${suffix}-arcade.tsv"
    local after_tsv="$OUT_DIR/${run_label}-arcade.tsv"
    if [[ ! -f "$baseline_tsv" ]]; then
      echo "validity_tsv	label=$run_label	valid=0	invalid_reason=missing_present_baseline	detail=$baseline_tsv"
      echo "$run_label gate failed: missing baseline trace $baseline_tsv" >&2
      return 9
    fi
    "$PRESENT_TRACE" compare "$baseline_tsv" "$after_tsv" --case "$run_label"
  fi
}

run_and_gate held-scroll FADE-VEL
run_and_gate turbo-hold FADE-TURBO
echo "$label preview 60fps gate passed"
