#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROFILE="$ROOT/scripts/profile-arcade-scroll.sh"
OUT_DIR="$ROOT/build/arcade-scroll-profiles"

usage() {
  cat <<'EOF'
Usage: scripts/gate-framebuffer-stream-55fps.sh [LABEL] [--secs N] [--deploy-device|--skip-build] [--self-test]

Runs no-subscriber, adaptive drain, and adaptive Analytics-display Arcade
turbo-hold profiles. Adaptive streaming remains opt-in; this gate does not
change production defaults.
EOF
}

label="FRAMEBUFFER-STREAM-$(date -u +%Y%m%dT%H%M%SZ)"
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

require_ge() {
  local label="$1" actual="$2" expected="$3"
  awk -v actual="$actual" -v expected="$expected" 'BEGIN { exit !(actual + 0 >= expected + 0) }' || {
    echo "$label failed: actual=$actual expected>=$expected" >&2
    return 1
  }
}

require_le() {
  local label="$1" actual="$2" expected="$3"
  awk -v actual="$actual" -v expected="$expected" 'BEGIN { exit !(actual + 0 <= expected + 0) }' || {
    echo "$label failed: actual=$actual expected<=$expected" >&2
    return 1
  }
}

require_eq() {
  local label="$1" actual="$2" expected="$3"
  if [[ "$actual" != "$expected" ]]; then
    echo "$label failed: actual=$actual expected=$expected" >&2
    return 1
  fi
}

validate_display_cadence() {
  local display="$1" cadence="$2"
  require_eq "Desktop build profile" "$(tsv_field "$display" build_profile)" release || return 1
  require_eq "Desktop display clock" "$(tsv_field "$display" clock)" macos-cadisplaylink || return 1
  require_eq "Desktop benchmark completion" "$(tsv_field "$display" completed)" 1 || return 1
  require_eq "Desktop benchmark validity" "$(tsv_field "$display" invalid_reason)" none || return 1
  require_eq "Rendering notifier ready" "$(tsv_field "$display" rendering_notifier_ready)" 1 || return 1
  require_eq "Desktop focused" "$(tsv_field "$display" focused)" 1 || return 1
  require_eq "Desktop occluded" "$(tsv_field "$display" occluded)" 0 || return 1
  require_eq "Desktop lost focus" "$(tsv_field "$display" lost_focus)" 0 || return 1
  require_eq "Desktop was occluded" "$(tsv_field "$display" was_occluded)" 0 || return 1
  require_ge "Analytics received fps" "$(tsv_field "$display" received_fps)" "${MISTER_STREAM_MIN_RECEIVED_FPS:-58}" || return 1
  require_ge "Analytics applied fps" "$(tsv_field "$display" applied_fps)" "${MISTER_STREAM_MIN_APPLIED_FPS:-55}" || return 1
  require_ge "Analytics rendered fps" "$(tsv_field "$display" rendered_fps)" "${MISTER_STREAM_MIN_RENDERED_FPS:-55}" || return 1
  require_le "Render latency p95" "$(tsv_field "$display" render_p95_ms)" "${MISTER_STREAM_MAX_RENDER_P95_MS:-50}" || return 1
  require_le "Render interval p95" "$(tsv_field "$cadence" interval_p95_us)" "${MISTER_STREAM_MAX_INTERVAL_P95_US:-20000}" || return 1
  require_eq "Render gaps over 34ms" "$(tsv_field "$cadence" gaps_over_34ms)" 0 || return 1
  require_eq "Consecutive render gaps over 20ms" "$(tsv_field "$cadence" max_consecutive_over_20ms)" 0 || return 1
  require_ge "Rendered frames per complete 500ms bucket" "$(tsv_field "$cadence" bucket_500ms_min)" "${MISTER_STREAM_MIN_500MS_RENDERED:-27}" || return 1
  local received coalesced coalescing_pct
  received="$(tsv_field "$display" received)"
  coalesced="$(tsv_field "$display" coalesced)"
  coalescing_pct="$(awk -v received="$received" -v coalesced="$coalesced" 'BEGIN { if (received == 0) print 100; else printf "%.3f", 100 * coalesced / received }')"
  require_le "Desktop coalescing percent" "$coalescing_pct" "${MISTER_STREAM_MAX_COALESCING_PCT:-10}" || return 1
}

run_self_test() {
  local good_display good_cadence burst_display burst_cadence
  good_display=$'framebuffer_display_bench_tsv\tsource=synthetic\tchrome=off\tclock=macos-cadisplaylink\tseconds=30\tbuild_profile=release\tcompleted=1\tinvalid_reason=none\treceived=1800\tapplied=1770\trendered=1740\treceived_fps=60.0\tapplied_fps=59.0\trendered_fps=58.0\tcoalesced=30\trender_p95_ms=12\trendering_notifier_ready=1\tfocused=1\toccluded=0\tlost_focus=0\twas_occluded=0'
  good_cadence=$'framebuffer_cadence_summary_tsv\tobserver=after-rendering\tsamples=1740\tinterval_p95_us=18000\tgaps_over_34ms=0\tmax_consecutive_over_20ms=0\tbucket_500ms_min=28'
  validate_display_cadence "$good_display" "$good_cadence" || {
    echo "valid smooth cadence fixture unexpectedly failed" >&2
    exit 1
  }

  burst_display="${good_display/rendered_fps=58.0/rendered_fps=56.0}"
  burst_cadence=$'framebuffer_cadence_summary_tsv\tobserver=after-rendering\tsamples=1680\tinterval_p95_us=29000\tgaps_over_34ms=8\tmax_consecutive_over_20ms=5\tbucket_500ms_min=19'
  if validate_display_cadence "$burst_display" "$burst_cadence" >/dev/null 2>&1; then
    echo "bursty 56fps cadence fixture unexpectedly passed" >&2
    exit 1
  fi
  echo "gate-framebuffer-stream-55fps self-test ok"
}

if [[ "$self_test" == "1" ]]; then
  run_self_test
  exit 0
fi

run_profile() {
  local run_label="$1" consumer="$2" scale="$3" build_mode="$4"
  local profile_secs=$((secs + 25))
  "$PROFILE" "$run_label" "$build_mode" --skip-boot-prelude --secs "$profile_secs" \
    --scenario turbo-hold --present-backend fpga-vblank-latch-hidden \
    --frame-pacing-policy vsync-integrity \
    --catalog-refresh off --stream-consumer "$consumer" \
    --stream-secs "$secs" --stream-scale "$scale"
}

echo "==> Build production Skia desktop benchmark"
cargo build --manifest-path "$ROOT/desktop/Cargo.toml" --locked --release --no-default-features \
  --features compiled-ui,skia-renderer

nosub_label="${label}-NOSUB"
drain_label="${label}-DRAIN"
display_label="${label}-DISPLAY"

run_profile "$nosub_label" none off "$deploy"
run_profile "$drain_label" null-drain adaptive --skip-build
run_profile "$display_label" desktop-display adaptive --skip-build

display_file="$OUT_DIR/${display_label}-framebuffer-stream.tsv"
drain_file="$OUT_DIR/${drain_label}-framebuffer-stream.tsv"
display_log="$OUT_DIR/${display_label}-arcade-scroll.log"

display="$(grep '^framebuffer_display_bench_tsv' "$display_file" | tail -n 1)"
cadence="$(grep '^framebuffer_cadence_summary_tsv' "$display_file" | tail -n 1)"
drain_row="$(grep '^framebuffer_stream_bench_tsv' "$drain_file" | tail -n 1)"
snapshot="$(grep '^framebuffer_stream_snapshot_tsv' "$display_log" | tail -n 1)"

validate_display_cadence "$display" "$cadence" || exit 20
require_ge "Producer drain fps" "$(tsv_field "$drain_row" fps)" "${MISTER_STREAM_MIN_DRAIN_FPS:-58}"

implementation="$(tsv_field "$snapshot" implementation)"
if [[ "$implementation" != "scalar" ]]; then
  echo "Framebuffer decimator gate failed: implementation=$implementation" >&2
  exit 22
fi
half_p95="$(tsv_field "$snapshot" half_snapshot_p95_us)"
require_le "Half snapshot p95" "$half_p95" "${MISTER_STREAM_MAX_HALF_P95_US:-4000}"
require_le "Half snapshot max" "$(tsv_field "$snapshot" half_snapshot_max_us)" "${MISTER_STREAM_MAX_HALF_MAX_US:-6000}"
require_le "Full snapshot p95" "$(tsv_field "$snapshot" full_snapshot_p95_us)" "${MISTER_STREAM_MAX_FULL_P95_US:-10000}"
require_le "Full snapshot max" "$(tsv_field "$snapshot" full_snapshot_max_us)" "${MISTER_STREAM_MAX_FULL_MAX_US:-15000}"

echo "framebuffer_stream_gate_tsv\tlabel=$label\trendered_fps=$(tsv_field "$display" rendered_fps)\tapplied_fps=$(tsv_field "$display" applied_fps)\tdrain_fps=$(tsv_field "$drain_row" fps)\trender_p95_ms=$(tsv_field "$display" render_p95_ms)\thalf_snapshot_p95_us=$half_p95\timplementation=scalar\tvalid=1"
