#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="$ROOT/scripts/profile-preview-scroll.sh"
OUT_DIR="$ROOT/build/preview-scroll-profiles"

secs=60
deploy_arg="--skip-build"
visual_captures=0
p99_work_us=14500
self_test=0
label=""

usage() {
  cat <<'EOF'
Usage: scripts/gate-preview-60fps.sh LABEL [--secs N] [--skip-build|--deploy-device] [--visual-captures N] [--p99-work-us N] [--self-test]

Runs the final Arcade preview fade pacing gate:
  - held-scroll fade
  - turbo-hold fade

Fails if either run has vsync fallback/timeout/error, non-zero max vsync miss
streak, or p99 work above the threshold. Reports work-over-budget outliers
separately so scheduler spikes do not hide p99 headroom.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs) secs="${2:-}"; shift 2 ;;
    --skip-build) deploy_arg="--skip-build"; shift ;;
    --deploy-device) deploy_arg="--deploy-device"; shift ;;
    --visual-captures) visual_captures="${2:-}"; shift 2 ;;
    --p99-work-us) p99_work_us="${2:-}"; shift 2 ;;
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
        required = "frame prepare_us slint_render_us custom_draw_us fb_present_us vsync_source vsync_miss_streak"
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
        printf "%d %d %d %d %d %d %d %d\n",
          n + 0, work_over + 0, vsync + 0, fallback + 0,
          timeout + 0, error + 0, other_source + 0, max_miss + 0
      }
    ' "$tsv"
  )"
  read -r frames work_over vsync fallback timeout error other_source max_miss <<<"$summary"
  if [[ "${frames:-0}" == missing-column:* ]]; then
    echo "$name gate failed: ${frames#missing-column:} column missing in $tsv" >&2
    rm -f "$works"
    return 9
  fi
  if [[ "$frames" -le 0 ]]; then
    echo "$name gate failed: no frames after frame 30 in $tsv" >&2
    rm -f "$works"
    return 9
  fi
  local p99_index p99_work
  p99_index=$((frames * 99 / 100))
  if [[ "$p99_index" -lt 1 ]]; then p99_index=1; fi
  p99_work="$(sort -n "$works" | awk -v idx="$p99_index" 'NR == idx { print; exit }')"
  rm -f "$works"

  echo "$name gate frames_after_30=$frames p99_work_us=$p99_work work_gt_16667=$work_over vsync=$vsync fallback=$fallback timeout=$timeout error=$error other_source=$other_source max_miss_streak=$max_miss"
  if [[ "$fallback" -ne 0 || "$timeout" -ne 0 || "$error" -ne 0 || "$other_source" -ne 0 || "$max_miss" -ne 0 || "$p99_work" -ge "$p99_work_us" ]]; then
    echo "$name gate failed" >&2
    return 9
  fi
}

write_self_test_trace() {
  local path="$1" work="$2" source="$3" miss="$4"
  {
    echo $'frame\tprepare_us\tslint_render_us\tcustom_draw_us\tfb_present_us\tvsync_source\tvsync_miss_streak'
    for frame in $(seq 0 40); do
      echo "${frame}"$'\t'"${work}"$'\t0\t0\t0\t'"${source}"$'\t'"${miss}"
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
}

run_and_gate held-scroll FADE-VEL
run_and_gate turbo-hold FADE-TURBO
echo "$label preview 60fps gate passed"
