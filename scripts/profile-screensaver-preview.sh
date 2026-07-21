#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Profile the real Settings -> Screensaver -> Preview screensaver path.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
REMOTE_BIN="/media/fat/mister-magik-dev/mister-magik-fb"
OUT_DIR="$HERE/build/screensaver-profiles"
SECS=60
DEPLOY=1
LABEL="screensaver-preview-$(date -u +%Y%m%dT%H%M%SZ)"

source "$HERE/scripts/lib/mister-supervision-lib.sh"
source "$HERE/scripts/lib/bench-context-lib.sh"

usage() {
  echo "Usage: scripts/profile-screensaver-preview.sh [LABEL] [--secs N] [--skip-build]"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --secs) SECS="${2:?--secs needs a positive integer}"; shift 2 ;;
    --skip-build) DEPLOY=0; shift ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    *) LABEL="$1"; shift ;;
  esac
done

[[ "$LABEL" =~ ^[A-Za-z0-9_.-]+$ ]] || { echo "invalid label: $LABEL" >&2; exit 2; }
[[ "$SECS" =~ ^[0-9]+$ && "$SECS" -gt 0 ]] || { echo "--secs must be positive" >&2; exit 2; }

mkdir -p "$OUT_DIR"
LOCAL_LOG="$OUT_DIR/$LABEL.log"
LOCAL_RESULT="$OUT_DIR/$LABEL-summary.tsv"
LOCAL_LATCH_BEFORE="$OUT_DIR/$LABEL-latch-before.log"
LOCAL_LATCH_AFTER="$OUT_DIR/$LABEL-latch-after.log"
LOCAL_PNG="$OUT_DIR/$LABEL.png"
LOCAL_JSON="$OUT_DIR/$LABEL.framebuffer.json"

if [[ "$DEPLOY" -eq 1 ]]; then
  "$HERE/scripts/agent" deploy-recipe launcher-bench-device
fi

BINARY_PATH="$HERE/apps/mister/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb"
DEPLOYED_SHA="$(bench_context_remote_sha256 "$MISTER" "$REMOTE_BIN" || true)"
if ! bench_context_require_binary_contract "$BINARY_PATH" "${DEPLOYED_SHA:-missing}" "ui,bench-tools" release-device launcher; then
  echo "screensaver benchmark binary contract verification failed" >&2
  exit 1
fi
SOURCE_FIELDS="$(bench_context_source_fields "$HERE")"
BINARY_FIELDS="$(bench_context_binary_fields release-device launcher ui,bench-tools "$BINARY_PATH" bench-tools verified "$DEPLOYED_SHA")"
printf 'run_context_tsv\tlabel=%s\tcommand=%s\tscenario=screensaver-show\tsecs=%s\tdeploy=%s\t%s\t%s\n' \
  "$LABEL" "scripts/profile-screensaver-preview.sh" "$SECS" "$DEPLOY" "$SOURCE_FIELDS" "$BINARY_FIELDS" >"$LOCAL_LOG"

RESTORE_ARMED=0
cleanup() {
  if [[ "$RESTORE_ARMED" -eq 1 ]]; then
    mister_restart_launcher >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT INT TERM

"$MISTER" run "'$REMOTE_BIN' fpga-latch-report" >"$LOCAL_LATCH_BEFORE"
mister_suspend_launcher
RESTORE_ARMED=1

"$MISTER" run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
rm -f /tmp/mister-magik-screensaver-preview.log
MISTER_LAUNCHER_START_SCREEN=screensaver \
MISTER_LAUNCHER_INPUT_SCRIPT_WAIT_FRAMES=1 \
MISTER_LAUNCHER_INPUT_SCRIPT='down,wait:30,down,wait:30,a' \
MISTER_PROFILE=summary \
'$REMOTE_BIN' ui launcher $((SECS + 5)) >/tmp/mister-magik-screensaver-preview.log 2>&1 &
PID=\$!
READY=0
i=0
while [ \$i -lt 200 ]; do
  if grep -q 'screensaver_startup_timing milestone=show_pressed' /tmp/mister-magik-screensaver-preview.log; then READY=1; break; fi
  kill -0 \$PID 2>/dev/null || break
  sleep 0.1
  i=\$((i + 1))
done
[ \$READY -eq 1 ] || { cat /tmp/mister-magik-screensaver-preview.log; exit 70; }
CLK_TCK=100
CPU_SUM=0
CPU_MAX=0
CPU_N=0
RSS_MAX=0
START=\$(awk '{print \$14+\$15}' /proc/\$PID/stat)
i=0
while [ \$i -lt $SECS ] && kill -0 \$PID 2>/dev/null; do
  BEFORE=\$(awk '{print \$14+\$15}' /proc/\$PID/stat)
  sleep 1
  AFTER=\$(awk '{print \$14+\$15}' /proc/\$PID/stat 2>/dev/null || echo \$BEFORE)
  CPU=\$(( (AFTER-BEFORE)*100/CLK_TCK ))
  RSS=\$(awk '/^VmRSS:/{print \$2}' /proc/\$PID/status 2>/dev/null || echo 0)
  case \"\$RSS\" in ''|*[!0-9]*) RSS=0 ;; esac
  CPU_SUM=\$((CPU_SUM + CPU)); CPU_N=\$((CPU_N + 1))
  [ \$CPU -gt \$CPU_MAX ] && CPU_MAX=\$CPU
  [ \$RSS -gt \$RSS_MAX ] && RSS_MAX=\$RSS
  i=\$((i + 1))
done
[ \$CPU_N -eq $SECS ] || { echo \"screensaver benchmark ended early samples=\$CPU_N expected=$SECS\"; exit 71; }
END=\$(awk '{print \$14+\$15}' /proc/\$PID/stat 2>/dev/null || echo \$START)
CPU_AVG=0; [ \$CPU_N -gt 0 ] && CPU_AVG=\$((CPU_SUM/CPU_N))
echo screensaver_bench_result samples=\$CPU_N cpu_avg_pct=\$CPU_AVG cpu_max_pct=\$CPU_MAX rss_max_kib=\$RSS_MAX cpu_ticks=\$((END-START))
wait \$PID
cat /tmp/mister-magik-screensaver-preview.log
" >>"$LOCAL_LOG"

"$MISTER" agent framebuffer-capture "$LOCAL_PNG" --json "$LOCAL_JSON" >/dev/null
"$MISTER" run "'$REMOTE_BIN' fpga-latch-report" >"$LOCAL_LATCH_AFTER"

latch_field() {
  local file="$1" field="$2"
  sed -n "s/.*${field}=\([0-9][0-9]*\).*/\1/p" "$file" | tail -1
}
flip_before="$(latch_field "$LOCAL_LATCH_BEFORE" flip_count)"
flip_after="$(latch_field "$LOCAL_LATCH_AFTER" flip_count)"
drop_before="$(latch_field "$LOCAL_LATCH_BEFORE" drop_count)"
drop_after="$(latch_field "$LOCAL_LATCH_AFTER" drop_count)"
for value in "$flip_before" "$flip_after" "$drop_before" "$drop_after"; do
  [[ "$value" =~ ^[0-9]+$ ]] || { echo "invalid FPGA latch evidence" >&2; exit 72; }
done
flip_delta=$(( (flip_after - flip_before + 65536) % 65536 ))
minimum_flips=$(( SECS * 59 ))
(( flip_delta >= minimum_flips )) || { echo "60 Hz latch gate failed flips=$flip_delta minimum=$minimum_flips" >&2; exit 73; }
(( drop_after == drop_before )) || { echo "FPGA drop gate failed before=$drop_before after=$drop_after" >&2; exit 74; }

{
  echo $'label\tsamples\tcpu_avg_pct\tcpu_max_pct\trss_max_kib\tflip_delta\tdrop_delta'
  awk -v label="$LABEL" -v flips="$flip_delta" -v drops="$((drop_after - drop_before))" '
    /screensaver_bench_result/ {
      for (i=1; i<=NF; i++) { split($i, a, "="); value[a[1]]=a[2] }
      printf "%s\t%s\t%s\t%s\t%s\t%d\t%d\n", label, value["samples"], value["cpu_avg_pct"], value["cpu_max_pct"], value["rss_max_kib"], flips, drops
    }
  ' "$LOCAL_LOG"
} >"$LOCAL_RESULT"

grep 'screensaver_startup_timing\|screensaver_frame_profile\|screensaver_bench_result' "$LOCAL_LOG" || true
echo "wrote $LOCAL_RESULT"
echo "wrote $LOCAL_LOG"
echo "wrote $LOCAL_PNG"
