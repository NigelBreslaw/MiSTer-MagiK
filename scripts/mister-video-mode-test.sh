#!/usr/bin/env bash
# Temporarily switch MiSTer.ini [Menu] video_mode for display-mode validation.
#
# Common PR4 flow:
#   scripts/mister-video-mode-test.sh set-960
#   scripts/mister-video-mode-test.sh magik-run static_ui 12
#   scripts/mister-video-mode-test.sh restore
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
AWK="$ROOT/scripts/mister-magik/set-menu-video-mode.awk"
WORK="$ROOT/build/mister-video-mode-test"
REMOTE_INI="/media/fat/MiSTer.ini"
REMOTE_BACKUP="/media/fat/MiSTer.ini.magik-mode-test.bak"
REMOTE_BIN="/media/fat/mister-magik/mister-magik-fb"
REMOTE_BENCH_REQUEST="/media/fat/mister-magik/bench-boot"
REMOTE_CONSOLE_TRACE="/tmp/mister-magik-console-scroll-trace.tsv"

usage() {
  cat <<EOF
Usage:
  scripts/mister-video-mode-test.sh set MODE
  scripts/mister-video-mode-test.sh set-960
  scripts/mister-video-mode-test.sh magik-run [SCENE] [SECS]
  scripts/mister-video-mode-test.sh magik-sweep [SECS]
  scripts/mister-video-mode-test.sh capture-console [SECS]
  scripts/mister-video-mode-test.sh stock-ui
  scripts/mister-video-mode-test.sh pattern [SECS] [normal|direct|none]
  scripts/mister-video-mode-test.sh run [SCENE] [SECS]
  scripts/mister-video-mode-test.sh restore

Examples:
  scripts/mister-video-mode-test.sh set-960
  scripts/mister-video-mode-test.sh magik-run static_ui 12
  scripts/mister-video-mode-test.sh magik-sweep 12
  scripts/mister-video-mode-test.sh capture-console 15
  scripts/mister-video-mode-test.sh restore
EOF
}

mister() {
  "$MISTER" "$@"
}

latest_local_backup() {
  ls -t "$WORK"/MiSTer.ini.*.bak 2>/dev/null | head -1
}

set_mode() {
  local mode="$1"
  mkdir -p "$WORK"
  local stamp
  stamp="$(date -u +%Y%m%dT%H%M%SZ)"
  local before="$WORK/MiSTer.ini.$stamp.bak"
  local after="$WORK/MiSTer.ini.$stamp.mode"

  echo "==> Backing up $REMOTE_INI"
  mister get "$REMOTE_INI" "$before"
  mister run "[ -f '$REMOTE_BACKUP' ] || cp '$REMOTE_INI' '$REMOTE_BACKUP'"

  echo "==> Writing [Menu] video_mode=$mode"
  awk -v mode="$mode" -f "$AWK" "$before" >"$after"
  mister put "$after" "$REMOTE_INI"

  echo "==> Rebooting into video_mode=$mode"
  mister reboot-wait
  echo "==> Mode set; run a scene with: scripts/mister-video-mode-test.sh magik-run static_ui 12"
}

restore_mode() {
  mkdir -p "$WORK"
  mister run "rm -f '$REMOTE_BENCH_REQUEST'; kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true"
  local backup
  backup="$(latest_local_backup || true)"
  if [[ -n "$backup" ]]; then
    echo "==> Restoring local backup $backup"
    mister put "$backup" "$REMOTE_INI"
  else
    echo "==> Restoring remote backup $REMOTE_BACKUP"
    mister run "test -f '$REMOTE_BACKUP'"
    mister run "cp '$REMOTE_BACKUP' '$REMOTE_INI'"
  fi
  echo "==> Rebooting after restore"
  mister reboot-wait
}

safe_scene() {
  case "$1" in
    demo|full_motion|static_ui|local_motion|console_scroll|launcher|controller_test) return 0 ;;
    *) return 1 ;;
  esac
}

write_bench_request() {
  local scene="$1"
  local secs="$2"
  safe_scene "$scene" || {
    echo "Unsupported scene for bench request: $scene" >&2
    exit 2
  }
  [[ "$secs" =~ ^[0-9]+$ ]] || {
    echo "Invalid seconds value: $secs" >&2
    exit 2
  }
  mister run "mkdir -p /media/fat/mister-magik; printf '%s %s\n' '$scene' '$secs' > '$REMOTE_BENCH_REQUEST'; rm -f '/tmp/mister-magik-bench-$scene.log' '$REMOTE_CONSOLE_TRACE'"
}

show_bench_log() {
  local scene="$1"
  local secs="$2"
  local wait_secs=$((secs + 8))
  if [[ "$secs" -eq 0 ]]; then
    wait_secs=5
  fi
  mister run "for i in \$(seq 1 20); do test -f '/tmp/mister-magik-bench-$scene.log' && break; sleep 1; done; sleep '$wait_secs'; sed -n '1,140p' '/tmp/mister-magik-bench-$scene.log' 2>/dev/null || true; echo '=== tail fps ==='; grep 'fps ~' '/tmp/mister-magik-bench-$scene.log' 2>/dev/null | tail -5 || true; grep '^done:' '/tmp/mister-magik-bench-$scene.log' 2>/dev/null || true; echo '=== main status ==='; cat /tmp/mister-magik/main-status.json 2>/dev/null || true"
}

magik_run() {
  local scene="${1:-static_ui}"
  local secs="${2:-12}"
  echo "==> Running through MiSTer MagiK bench boot scene=$scene secs=$secs"
  write_bench_request "$scene" "$secs"
  mister reboot-wait
  show_bench_log "$scene" "$secs"
}

magik_sweep() {
  local secs="${1:-12}"
  local scenes=(static_ui full_motion local_motion console_scroll demo)
  for scene in "${scenes[@]}"; do
    echo "=== MiSTer MagiK sweep scene=$scene secs=$secs ==="
    write_bench_request "$scene" "$secs"
    mister reboot-wait
    show_bench_log "$scene" "$secs"
  done
}

analyze_console_trace() {
  local trace="$1"
  awk '
BEGIN {
  print "=== console_scroll trace summary ==="
}
NR == 1 { next }
{
  bucket = ($2 < 10000) ? "first10s" : "after10s"
  n[bucket]++
  frame_wall[bucket] += $10
  vsync_wait[bucket] += $7
  fb_copy[bucket] += $8
  fb_hash[bucket] += $13
  if ($10 > max_wall[bucket]) max_wall[bucket] = $10
  if ($8 > max_copy[bucket]) max_copy[bucket] = $8
  if ($7 < 1000) low_vsync[bucket]++
  if ($10 > 20000) slow_wall[bucket]++
  if ($11 > $12) copy_over_budget[bucket]++
  if (last_hash != "" && $14 == last_hash) duplicate_hash[bucket]++
  last_hash = $14
}
END {
  for (i = 1; i <= 2; i++) {
    bucket = (i == 1) ? "first10s" : "after10s"
    if (n[bucket] == 0) {
      print bucket ": no frames"
      continue
    }
    printf "%s: frames=%d avg_wall_us=%d max_wall_us=%d avg_vsync_wait_us=%d avg_fb_copy_us=%d max_fb_copy_us=%d slow_wall_gt20ms=%d low_vsync_lt1ms=%d copy_over_budget=%d duplicate_hash=%d\n",
      bucket,
      n[bucket],
      frame_wall[bucket] / n[bucket],
      max_wall[bucket],
      vsync_wait[bucket] / n[bucket],
      fb_copy[bucket] / n[bucket],
      max_copy[bucket],
      slow_wall[bucket],
      low_vsync[bucket],
      copy_over_budget[bucket],
      duplicate_hash[bucket]
  }
}
' "$trace"
}

capture_console() {
  local secs="${1:-15}"
  [[ "$secs" =~ ^[1-9][0-9]*$ ]] || {
    echo "capture-console needs a positive seconds value" >&2
    exit 2
  }
  local stamp dir wait_secs
  stamp="$(date -u +%Y%m%dT%H%M%SZ)"
  dir="$WORK/captures/console-scroll-$stamp"
  wait_secs=$((secs + 8))
  mkdir -p "$dir"

  echo "==> Capturing console_scroll cold start secs=$secs -> $dir"
  write_bench_request console_scroll "$secs"
  mister reboot-wait
  mister run "for i in \$(seq 1 20); do test -f '/tmp/mister-magik-bench-console_scroll.log' && break; sleep 1; done; sleep '$wait_secs'"
  mister get /tmp/mister-magik-bench-console_scroll.log "$dir/console_scroll.log" || true
  mister get "$REMOTE_CONSOLE_TRACE" "$dir/console-scroll-trace.tsv" || true
  mister get /tmp/mister-magik/main-status.json "$dir/main-status.json" || true

  if [[ -f "$dir/console-scroll-trace.tsv" ]]; then
    analyze_console_trace "$dir/console-scroll-trace.tsv" | tee "$dir/summary.txt"
  else
    echo "trace missing: $dir/console-scroll-trace.tsv" | tee "$dir/summary.txt"
  fi
  echo "==> Capture files: $dir"
}

stock_ui_probe() {
  mkdir -p "$WORK"
  local stamp before after
  stamp="$(date -u +%Y%m%dT%H%M%SZ)"
  before="$WORK/MiSTer.ini.$stamp.before-stock-ui"
  after="$WORK/MiSTer.ini.$stamp.stock-ui"

  echo "==> Backing up $REMOTE_INI"
  mister get "$REMOTE_INI" "$before"

  echo "==> Commenting main=MiSTer_MagiK so stock MiSTer owns the menu"
  awk '
    {
      sub(/\r$/, "", $0)
      if ($0 == "main=MiSTer_MagiK") print ";main=MiSTer_MagiK ; stock UI video-mode probe"
      else print
    }
  ' "$before" >"$after"
  mister put "$after" "$REMOTE_INI"

  echo "==> Rebooting into stock MiSTer for display compatibility check"
  mister reboot-wait
  echo "==> Check the stock MiSTer OSD. Then run: scripts/mister-video-mode-test.sh pattern 0 normal"
}

pause_stock_mister() {
  mister run "kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true; if pidof MiSTer >/dev/null 2>&1; then kill -STOP \$(pidof MiSTer); fi"
}

run_pattern() {
  local secs="${1:-0}"
  local route="${2:-normal}"
  echo "==> Running simple framebuffer pattern secs=$secs route=$route"
  pause_stock_mister
  mister run "'$REMOTE_BIN' fb-current '$secs' '$route' >/tmp/mister-video-mode-pattern.log 2>&1 & echo pattern_pid=\$!; sleep 2; sed -n '1,100p' /tmp/mister-video-mode-pattern.log"
}

run_scene() {
  local scene="${1:-static_ui}"
  local secs="${2:-0}"
  echo "==> Running $scene for secs=$secs"
  pause_stock_mister
  mister run "'$REMOTE_BIN' ui '$scene' '$secs' >/tmp/mister-video-mode-test-$scene.log 2>&1 & echo ui_pid=\$!; sleep 4; sed -n '1,120p' /tmp/mister-video-mode-test-$scene.log"
}

case "${1:-}" in
  set)
    [[ $# -eq 2 ]] || { usage; exit 2; }
    set_mode "$2"
    ;;
  set-960)
    set_mode "960,540,60"
    ;;
  magik-run)
    shift
    magik_run "$@"
    ;;
  magik-sweep)
    shift
    magik_sweep "$@"
    ;;
  capture-console)
    shift
    capture_console "$@"
    ;;
  stock-ui)
    stock_ui_probe
    ;;
  pattern)
    shift
    run_pattern "$@"
    ;;
  run)
    shift
    run_scene "$@"
    ;;
  restore)
    restore_mode
    ;;
  -h|--help|help|"")
    usage
    ;;
  *)
    echo "Unknown command: $1" >&2
    usage >&2
    exit 2
    ;;
esac
