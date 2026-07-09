#!/usr/bin/env bash
# Gate max-speed Home launcher system-row scrolling with zero over-budget frames.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
OUT_DIR="$HERE/build/launcher-home-scroll-profiles"
REMOTE_ENV="/media/fat/mister-magik/launcher.env"
REMOTE_LOG="/tmp/mister-magik-slint.log"

label="launcher-home-max-scroll-$(date -u +%Y%m%dT%H%M%SZ)"
secs="30"
deploy="skip"
ui_fb_size="${MISTER_UI_FB_SIZE:-auto}"
present_delay_us="${MISTER_FB_PRESENT_DELAY_US:-0}"
catalog_refresh="${MISTER_CATALOG_REFRESH:-off}"
present_backend="${MISTER_PRESENT_BACKEND:-fpga-vblank-latch-hidden}"

usage() {
  cat <<'EOF'
Usage: scripts/gate-launcher-home-max-scroll-zero-drops.sh [LABEL] [--secs N] [--deploy-device|--skip-build] [--ui-fb-size auto|960x540|1280x720] [--present-delay-us N] [--catalog-refresh default|off|force] [--present-backend BACKEND]

Runs the real launcher Home screen horizontal system row with the
home-repeat-hold scenario for 30s by default. The scenario holds left/right
through the normal launcher input path, including the real repeat delay and
80ms repeat cadence, and reverses at the ends of the system row.

The default backend is fpga-vblank-latch-hidden. For /dev/fb0 the gate fails if
any measured frame after warmup has wall_us > 16667 or loop_delta_us > 16667.
For the FPGA latch backend, the gate fails on latch-visible evidence: deadline
misses, repeated buffers, sampled flip-counter gaps, unsupported status, or
passive FPGA drop_count > 0. It still reports wall/loop overages as scheduler
wake jitter.

Default: --skip-build, useful when a bench-tools MagiK binary is already
deployed. Use --deploy-device to build and deploy one first.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs) secs="${2:?}"; shift 2 ;;
    --deploy-device) deploy="device"; shift ;;
    --skip-build) deploy="skip"; shift ;;
    --ui-fb-size) ui_fb_size="${2:?}"; shift 2 ;;
    --present-delay-us) present_delay_us="${2:?}"; shift 2 ;;
    --catalog-refresh) catalog_refresh="${2:?}"; shift 2 ;;
    --present-backend) present_backend="${2:?}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    --*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) label="$1"; shift ;;
  esac
done

if [[ ! "$label" =~ ^[A-Za-z0-9_.-]+$ ]]; then
  echo "label must contain only letters, numbers, _, ., or -" >&2
  exit 2
fi
if [[ ! "$secs" =~ ^[0-9]+$ || "$secs" -lt 1 ]]; then
  echo "--secs must be a positive integer" >&2
  exit 2
fi
case "$ui_fb_size" in
  auto|960x540|1280x720) ;;
  *) echo "--ui-fb-size must be auto, 960x540, or 1280x720" >&2; exit 2 ;;
esac
if [[ ! "$present_delay_us" =~ ^[0-9]+$ ]]; then
  echo "--present-delay-us must be a non-negative integer" >&2
  exit 2
fi
case "$catalog_refresh" in
  default|off|force) ;;
  *) echo "--catalog-refresh must be default, off, or force" >&2; exit 2 ;;
esac
mkdir -p "$OUT_DIR"
env_file="$(mktemp "${TMPDIR:-/tmp}/mister-magik-home-scroll-env.XXXXXX")"
remote_trace="/tmp/${label}-launcher-home-scroll.tsv"
trace="$OUT_DIR/${label}-launcher-home-scroll.tsv"
log="$OUT_DIR/${label}-launcher-home-scroll.log"
status_json="$OUT_DIR/${label}-launcher-home-scroll.status.json"
drop_report="$OUT_DIR/${label}-launcher-home-scroll-drops.tsv"
fpga_latch_before="$OUT_DIR/${label}-fpga-latch-before.log"
fpga_latch_after="$OUT_DIR/${label}-fpga-latch-after.log"

cleanup() {
  rm -f "$env_file"
  "$MISTER" run "rm -f '$REMOTE_ENV'; if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; fi" >/dev/null 2>&1 || true
}
trap cleanup EXIT

case "$deploy" in
  device) "$HERE/scripts/deploy-rust.sh" --device --ui-scope launcher --bench-tools --diagnostics ;;
  skip) : ;;
esac

{
  printf 'export MISTER_UI_FB_SIZE=%q\n' "$ui_fb_size"
  printf 'export MISTER_FB_PRESENT_DELAY_US=%q\n' "$present_delay_us"
  printf 'export MISTER_CATALOG_REFRESH=%q\n' "$catalog_refresh"
  if [[ -n "$present_backend" ]]; then
    printf 'export MISTER_PRESENT_BACKEND=%q\n' "$present_backend"
  fi
  printf 'export MISTER_LAUNCHER_START_SCREEN=home\n'
  printf 'export MISTER_LAUNCHER_LOCK_SCREEN=home\n'
  printf 'export MISTER_LAUNCHER_BENCH_SCENARIO=home-repeat-hold\n'
  printf 'export MISTER_PREVIEW_SCROLL_TRACE_SECS=%q\n' "$secs"
  printf 'export MISTER_PREVIEW_SCROLL_TRACE=%q\n' "$remote_trace"
} >"$env_file"

rm -f "$trace" "$log" "$status_json" "$drop_report" "$fpga_latch_before" "$fpga_latch_after"
echo "==> Capture supervised launcher Home system-row home-repeat-hold secs=$secs label=$label deploy=$deploy ui_fb_size=$ui_fb_size present_delay_us=$present_delay_us catalog_refresh=$catalog_refresh"
if [[ "$present_backend" == "fpga-vblank-latch-hidden" ]]; then
  "$MISTER" run "'/media/fat/mister-magik/mister-magik-fb' fpga-latch-report" >"$fpga_latch_before"
  echo "wrote $fpga_latch_before"
fi
"$MISTER" run "rm -f '$remote_trace' '$REMOTE_LOG'; sync" >/dev/null
"$MISTER" put "$env_file" "$REMOTE_ENV" >/dev/null
"$MISTER" run "if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd" >/dev/null
sleep $((secs + 7))

if ! "$MISTER" get "$remote_trace" "$trace" >/dev/null; then
  "$MISTER" get "$REMOTE_LOG" "$log" >/dev/null || true
  echo "launcher Home max-scroll trace failed; see $log" >&2
  if [[ -s "$log" ]] && ! grep -q 'launcher_bench_scenario=home-repeat-hold' "$log"; then
    echo "trace was not armed; the deployed MagiK binary probably lacks --bench-tools, so rerun with --deploy-device" >&2
  fi
  exit 1
fi
"$MISTER" get "$REMOTE_LOG" "$log" >/dev/null || true
"$MISTER" status --json >"$status_json" 2>/dev/null || true
if [[ "$present_backend" == "fpga-vblank-latch-hidden" ]]; then
  "$MISTER" run "'/media/fat/mister-magik/mister-magik-fb' fpga-latch-report" >"$fpga_latch_after" || true
  echo "wrote $fpga_latch_after"
fi

echo "wrote $trace"
echo "wrote $log"
echo "wrote $status_json"

analyze_args=()
if [[ -n "$present_backend" ]]; then
  analyze_args+=(--expect-backend "$present_backend")
fi
if [[ "$present_backend" == "fpga-vblank-latch-hidden" ]]; then
  analyze_args+=(--fpga-latch-report-before "$fpga_latch_before")
  analyze_args+=(--fpga-latch-report-after "$fpga_latch_after")
fi

set +e
"$HERE/scripts/analyze-max-scroll-drops.py" "$trace" \
  --label "$label" \
  --status-json "$status_json" \
  "${analyze_args[@]}" | tee "$drop_report"
drop_status=${PIPESTATUS[0]}
set -e

echo "wrote $drop_report"
exit "$drop_status"
