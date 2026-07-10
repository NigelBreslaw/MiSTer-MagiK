#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="$ROOT/scripts/profile-arcade-scroll.sh"
OUT_DIR="$ROOT/build/arcade-scroll-profiles"

usage() {
  cat <<'EOF'
Usage: scripts/gate-framebuffer-stream-55fps.sh [LABEL] [--secs N] [--deploy-device|--skip-build]

Runs no-subscriber, scalar adaptive, NEON drain, and NEON Analytics-display
Arcade turbo-hold profiles. Adaptive streaming remains opt-in; this gate does
not change production defaults.
EOF
}

label="NEON-STREAM-$(date -u +%Y%m%dT%H%M%SZ)"
secs="30"
deploy="--skip-build"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs) secs="${2:?--secs needs N}"; shift 2 ;;
    --deploy-device) deploy="--deploy-device"; shift ;;
    --skip-build) deploy="--skip-build"; shift ;;
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

require_ge() {
  local label="$1" actual="$2" expected="$3"
  awk -v actual="$actual" -v expected="$expected" 'BEGIN { exit !(actual + 0 >= expected + 0) }' || {
    echo "$label failed: actual=$actual expected>=$expected" >&2
    exit 20
  }
}

require_le() {
  local label="$1" actual="$2" expected="$3"
  awk -v actual="$actual" -v expected="$expected" 'BEGIN { exit !(actual + 0 <= expected + 0) }' || {
    echo "$label failed: actual=$actual expected<=$expected" >&2
    exit 21
  }
}

run_profile() {
  local run_label="$1" consumer="$2" simd="$3" scale="$4" build_mode="$5"
  local profile_secs=$((secs + 25))
  "$PROFILE" "$run_label" "$build_mode" --skip-boot-prelude --secs "$profile_secs" \
    --scenario turbo-hold --present-backend fpga-vblank-latch-hidden \
    --frame-pacing-policy vsync-integrity \
    --catalog-refresh off --stream-consumer "$consumer" \
    --stream-secs "$secs" --stream-scale "$scale" --stream-simd "$simd"
}

echo "==> Build production Skia desktop benchmark"
cargo build --manifest-path "$ROOT/desktop/Cargo.toml" --locked --features skia-renderer

nosub_label="${label}-NOSUB"
scalar_label="${label}-SCALAR"
drain_label="${label}-AUTO-DRAIN"
auto_label="${label}-AUTO-DISPLAY"

run_profile "$nosub_label" none auto off "$deploy"
run_profile "$scalar_label" desktop-display scalar adaptive --skip-build
run_profile "$drain_label" null-drain auto adaptive --skip-build
run_profile "$auto_label" desktop-display auto adaptive --skip-build

scalar_display_file="$OUT_DIR/${scalar_label}-framebuffer-stream.tsv"
auto_display_file="$OUT_DIR/${auto_label}-framebuffer-stream.tsv"
drain_file="$OUT_DIR/${drain_label}-framebuffer-stream.tsv"
scalar_log="$OUT_DIR/${scalar_label}-arcade-scroll.log"
auto_log="$OUT_DIR/${auto_label}-arcade-scroll.log"

scalar_display="$(grep '^framebuffer_display_bench_tsv' "$scalar_display_file" | tail -n 1)"
auto_display="$(grep '^framebuffer_display_bench_tsv' "$auto_display_file" | tail -n 1)"
drain_row="$(grep '^framebuffer_stream_bench_tsv' "$drain_file" | tail -n 1)"
scalar_snapshot="$(grep '^framebuffer_stream_snapshot_tsv' "$scalar_log" | tail -n 1)"
auto_snapshot="$(grep '^framebuffer_stream_snapshot_tsv' "$auto_log" | tail -n 1)"

require_ge "Analytics rendered fps" "$(tsv_field "$auto_display" rendered_fps)" "${MISTER_STREAM_MIN_RENDERED_FPS:-55}"
require_ge "Analytics applied fps" "$(tsv_field "$auto_display" applied_fps)" "${MISTER_STREAM_MIN_APPLIED_FPS:-55}"
require_ge "Producer drain fps" "$(tsv_field "$drain_row" fps)" "${MISTER_STREAM_MIN_DRAIN_FPS:-58}"
require_le "Render latency p95" "$(tsv_field "$auto_display" render_p95_ms)" "${MISTER_STREAM_MAX_RENDER_P95_MS:-50}"
require_ge "Rendering notifier" "$(tsv_field "$auto_display" notifier_supported)" 1

auto_impl="$(tsv_field "$auto_snapshot" implementation)"
if [[ "$auto_impl" != "neon" ]]; then
  echo "NEON dispatch gate failed: implementation=$auto_impl" >&2
  exit 22
fi
require_le "Half snapshot p95" "$(tsv_field "$auto_snapshot" half_snapshot_p95_us)" "${MISTER_STREAM_MAX_HALF_P95_US:-4000}"
require_le "Half snapshot max" "$(tsv_field "$auto_snapshot" half_snapshot_max_us)" "${MISTER_STREAM_MAX_HALF_MAX_US:-6000}"
require_le "Full snapshot p95" "$(tsv_field "$auto_snapshot" full_snapshot_p95_us)" "${MISTER_STREAM_MAX_FULL_P95_US:-10000}"
require_le "Full snapshot max" "$(tsv_field "$auto_snapshot" full_snapshot_max_us)" "${MISTER_STREAM_MAX_FULL_MAX_US:-15000}"

scalar_p95="$(tsv_field "$scalar_snapshot" half_snapshot_p95_us)"
auto_p95="$(tsv_field "$auto_snapshot" half_snapshot_p95_us)"
speedup="$(awk -v scalar="$scalar_p95" -v neon="$auto_p95" 'BEGIN { if (neon == 0) print 0; else printf "%.2f", scalar / neon }')"
require_ge "NEON scalar speedup" "$speedup" "${MISTER_STREAM_MIN_NEON_SPEEDUP:-1.5}"

echo "framebuffer_stream_gate_tsv\tlabel=$label\trendered_fps=$(tsv_field "$auto_display" rendered_fps)\tapplied_fps=$(tsv_field "$auto_display" applied_fps)\tdrain_fps=$(tsv_field "$drain_row" fps)\trender_p95_ms=$(tsv_field "$auto_display" render_p95_ms)\thalf_snapshot_p95_us=$auto_p95\tneon_speedup=$speedup\tvalid=1"
