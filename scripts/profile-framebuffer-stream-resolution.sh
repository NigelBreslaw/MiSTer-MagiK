#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="$ROOT/scripts/profile-arcade-scroll.sh"
OUT_DIR="$ROOT/build/arcade-scroll-profiles"

usage() {
  cat <<'EOF'
Usage: scripts/profile-framebuffer-stream-resolution.sh [LABEL] [--secs N] [--deploy-device|--skip-build] [--self-test]

Runs matched half, full, and adaptive 30-second null-drain/display profiles,
plus an adaptive motion/settle refinement profile. Production defaults are not
changed.
EOF
}

label="STREAM-RES-$(date -u +%Y%m%dT%H%M%SZ)"
secs="30"
deploy="--skip-build"
self_test="0"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs) secs="${2:?--secs needs N}"; shift 2 ;;
    --deploy-device) deploy="--deploy-device"; shift ;;
    --skip-build) deploy="--skip-build"; shift ;;
    --self-test) self_test="1"; shift ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) label="$1"; shift ;;
  esac
done

if [[ ! "$secs" =~ ^[0-9]+$ || "$secs" -lt 10 ]]; then
  echo "--secs must be an integer >= 10" >&2
  exit 2
fi
if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "LABEL must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi

tsv_field() {
  local line="$1" key="$2"
  awk -F '\t' -v key="$key" '
    {
      for (i = 1; i <= NF; i++) {
        split($i, pair, "=")
        if (pair[1] == key) { print substr($i, length(key) + 2); exit }
      }
    }
  ' <<<"$line"
}

require_refinement() {
  local snapshot="$1"
  local count width height late_max
  count="$(tsv_field "$snapshot" adaptive_refinements)"
  width="$(tsv_field "$snapshot" last_refinement_width)"
  height="$(tsv_field "$snapshot" last_refinement_height)"
  late_max="$(tsv_field "$snapshot" refinement_late_max_us)"
  [[ "$count" =~ ^[0-9]+$ && "$count" -ge 1 ]] || {
    echo "adaptive refinement gate failed: count=$count" >&2
    return 1
  }
  [[ "$width" == "960" && "$height" == "540" ]] || {
    echo "adaptive refinement geometry failed: ${width}x${height}" >&2
    return 1
  }
  awk -v actual="$late_max" 'BEGIN { exit !(actual + 0 <= 15000) }' || {
    echo "adaptive refinement lateness failed: actual=${late_max}us expected<=15000us" >&2
    return 1
  }
}

run_self_test() {
  local valid missing late
  valid=$'framebuffer_stream_snapshot_tsv\tadaptive_refinements=2\trefinement_late_max_us=12000\tlast_refinement_width=960\tlast_refinement_height=540'
  missing=$'framebuffer_stream_snapshot_tsv\tadaptive_refinements=0\trefinement_late_max_us=0\tlast_refinement_width=0\tlast_refinement_height=0'
  late=$'framebuffer_stream_snapshot_tsv\tadaptive_refinements=1\trefinement_late_max_us=16000\tlast_refinement_width=960\tlast_refinement_height=540'
  require_refinement "$valid"
  if require_refinement "$missing" >/dev/null 2>&1; then
    echo "missing refinement fixture unexpectedly passed" >&2
    exit 1
  fi
  if require_refinement "$late" >/dev/null 2>&1; then
    echo "late refinement fixture unexpectedly passed" >&2
    exit 1
  fi
  echo "profile-framebuffer-stream-resolution self-test ok"
}

if [[ "$self_test" == "1" ]]; then
  run_self_test
  exit 0
fi

run_profile() {
  local run_label="$1" consumer="$2" scale="$3" scenario="$4" build_mode="$5"
  "$PROFILE" "$run_label" "$build_mode" --skip-boot-prelude --secs "$((secs + 25))" \
    --scenario "$scenario" --present-backend fpga-vblank-latch-hidden \
    --frame-pacing-policy vsync-integrity --catalog-refresh off \
    --stream-consumer "$consumer" --stream-secs "$secs" \
    --stream-scale "$scale"
}

echo "==> Build release null-drain benchmark"
cargo build --manifest-path "$ROOT/desktop/Cargo.toml" --locked --release
echo "==> Build production Skia desktop benchmark"
cargo build --manifest-path "$ROOT/desktop/Cargo.toml" --locked --release \
  --no-default-features --features compiled-ui,skia-renderer

run_profile "${label}-NOSUB" none off turbo-hold "$deploy"
for scale in half full adaptive; do
  run_profile "${label}-${scale^^}-DRAIN" null-drain "$scale" turbo-hold --skip-build
  run_profile "${label}-${scale^^}-DISPLAY" desktop-display "$scale" turbo-hold --skip-build
done
run_profile "${label}-ADAPTIVE-REFINE" desktop-display adaptive human-turbo-hold --skip-build

summary="$OUT_DIR/${label}-framebuffer-resolution.tsv"
: >"$summary"
for scale in half full adaptive; do
  drain_label="${label}-${scale^^}-DRAIN"
  display_label="${label}-${scale^^}-DISPLAY"
  drain_row="$(grep '^framebuffer_stream_bench_tsv' "$OUT_DIR/${drain_label}-framebuffer-stream.tsv" | tail -n 1)"
  display_row="$(grep '^framebuffer_display_bench_tsv' "$OUT_DIR/${display_label}-framebuffer-stream.tsv" | tail -n 1)"
  snapshot_row="$(grep '^framebuffer_stream_snapshot_tsv' "$OUT_DIR/${display_label}-arcade-scroll.log" | tail -n 1)"
  printf 'framebuffer_resolution_profile_tsv\tlabel=%s\tscale=%s\tseconds=%s\tpayload_bytes=%s\traw_bytes=%s\ttransport_fps=%s\treceived_fps=%s\tapplied_fps=%s\trendered_fps=%s\tcoalesced=%s\tsnapshot_p95_us=%s\tsnapshot_max_us=%s\thalf_snapshot_p95_us=%s\tfull_snapshot_p95_us=%s\tlatch_gate=profile-pass\n' \
    "$label" "$scale" "$secs" \
    "$(tsv_field "$drain_row" avg_payload_bytes)" \
    "$(tsv_field "$drain_row" avg_raw_bytes)" \
    "$(tsv_field "$drain_row" fps)" \
    "$(tsv_field "$display_row" received_fps)" \
    "$(tsv_field "$display_row" applied_fps)" \
    "$(tsv_field "$display_row" rendered_fps)" \
    "$(tsv_field "$display_row" coalesced)" \
    "$(tsv_field "$snapshot_row" snapshot_p95_us)" \
    "$(tsv_field "$snapshot_row" snapshot_max_us)" \
    "$(tsv_field "$snapshot_row" half_snapshot_p95_us)" \
    "$(tsv_field "$snapshot_row" full_snapshot_p95_us)" | tee -a "$summary"
done

refinement_snapshot="$(grep '^framebuffer_stream_snapshot_tsv' "$OUT_DIR/${label}-ADAPTIVE-REFINE-arcade-scroll.log" | tail -n 1)"
require_refinement "$refinement_snapshot"
printf 'framebuffer_adaptive_refinement_tsv\tlabel=%s\trefinements=%s\tlate_p95_us=%s\tlate_max_us=%s\twidth=%s\theight=%s\tvalid=1\n' \
  "$label" \
  "$(tsv_field "$refinement_snapshot" adaptive_refinements)" \
  "$(tsv_field "$refinement_snapshot" refinement_late_p95_us)" \
  "$(tsv_field "$refinement_snapshot" refinement_late_max_us)" \
  "$(tsv_field "$refinement_snapshot" last_refinement_width)" \
  "$(tsv_field "$refinement_snapshot" last_refinement_height)" | tee -a "$summary"

echo "wrote $summary"
