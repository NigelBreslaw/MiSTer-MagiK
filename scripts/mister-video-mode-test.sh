#!/usr/bin/env bash
# Temporarily switch MiSTer.ini [Menu] video_mode for display-mode validation.
#
# Common PR4 flow:
#   scripts/mister-video-mode-test.sh set-960
#   scripts/mister-video-mode-test.sh magik-run static_ui 12
#   scripts/mister-video-mode-test.sh restore
#
# Common PR5 flow:
#   scripts/mister-video-mode-test.sh sweep-list
#   scripts/mister-video-mode-test.sh sweep-mode 960 static_ui
#   # visually verify, then repeat for the next label
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
  scripts/mister-video-mode-test.sh sweep-list
  scripts/mister-video-mode-test.sh sweep-mode LABEL [SCENE]
  scripts/mister-video-mode-test.sh stock-ui-mode LABEL
  scripts/mister-video-mode-test.sh stock-ui-auto
  scripts/mister-video-mode-test.sh stock-ui
  scripts/mister-video-mode-test.sh pattern [SECS] [normal|direct|none]
  scripts/mister-video-mode-test.sh run [SCENE] [SECS]
  scripts/mister-video-mode-test.sh restore

Examples:
  scripts/mister-video-mode-test.sh set-960
  scripts/mister-video-mode-test.sh magik-run static_ui 12
  scripts/mister-video-mode-test.sh magik-sweep 12
  scripts/mister-video-mode-test.sh capture-console 15
  scripts/mister-video-mode-test.sh sweep-list
  scripts/mister-video-mode-test.sh sweep-mode 720p static_ui
  scripts/mister-video-mode-test.sh stock-ui-mode high
  scripts/mister-video-mode-test.sh stock-ui-auto
  scripts/mister-video-mode-test.sh restore
EOF
}

mister() {
  "$MISTER" "$@"
}

latest_local_backup() {
  ls -t "$WORK"/MiSTer.ini.*.bak 2>/dev/null | head -1
}

mode_value_for_label() {
  case "$1" in
    auto|native|edid) echo "auto" ;;
    low|480p|640x480) echo "6" ;;
    960|540p|960x540) echo "960,540,60" ;;
    720p|1280x720) echo "0" ;;
    1080p|1920x1080) echo "8" ;;
    high|1440p|2560x1440) echo "14" ;;
    *,*,*) echo "$1" ;;
    *)
      echo "Unknown mode label: $1" >&2
      return 1
      ;;
  esac
}

sweep_list() {
  cat <<EOF
Representative HDMI mode sweep labels:
  auto    -> EDID/native    (comment [Menu] video_mode and let MiSTer choose)
  low     -> 6              (MiSTer preset: 640x480@60)
  960     -> 960,540,60
  720p    -> 0              (MiSTer preset: 1280x720@60)
  1080p   -> 8              (MiSTer preset: 1920x1080@60)
  high    -> 14             (MiSTer pixel-repetition preset, optional)

Run one visual checkpoint at a time:
  scripts/mister-video-mode-test.sh sweep-mode auto static_ui
  scripts/mister-video-mode-test.sh sweep-mode low static_ui
  scripts/mister-video-mode-test.sh sweep-mode 960 static_ui
  scripts/mister-video-mode-test.sh sweep-mode 720p full_motion

Use the preset labels for standard HDMI modes. Custom WIDTH,HEIGHT,REFRESH
values ask MiSTer to synthesize a CVT mode, which may not match stock timings.
The high preset is display-dependent. Stock MiSTer was also glitchy on the
current TV with both preset 14 and calculated 2560,1440,60, while EDID/native
auto mode selected stable 1080p. Treat high failure as unsupported
hardware/timing unless stock MiSTer proves otherwise.

Each run backs up MiSTer.ini, writes the mode, reboots through MiSTer_MagiK,
starts the selected benchmark indefinitely, captures a PNG using detected fb
dimensions, and leaves the scene on screen for visual verification.
EOF
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

write_mode_no_reboot() {
  local mode="$1"
  local out_dir="$2"
  mkdir -p "$WORK" "$out_dir"
  local stamp
  stamp="$(date -u +%Y%m%dT%H%M%SZ)"
  local before="$out_dir/MiSTer.ini.$stamp.bak"
  local after="$out_dir/MiSTer.ini.$stamp.mode"

  echo "==> Backing up $REMOTE_INI"
  mister get "$REMOTE_INI" "$before"
  cp "$before" "$WORK/MiSTer.ini.$stamp.bak"
  mister run "[ -f '$REMOTE_BACKUP' ] || cp '$REMOTE_INI' '$REMOTE_BACKUP'"

  echo "==> Writing [Menu] video_mode=$mode"
  awk -v mode="$mode" -f "$AWK" "$before" >"$after"
  mister put "$after" "$REMOTE_INI"
}

write_auto_no_reboot() {
  local out_dir="$1"
  mkdir -p "$WORK" "$out_dir"
  local stamp
  stamp="$(date -u +%Y%m%dT%H%M%SZ)"
  local before="$out_dir/MiSTer.ini.$stamp.bak"
  local after="$out_dir/MiSTer.ini.$stamp.auto"

  echo "==> Backing up $REMOTE_INI"
  mister get "$REMOTE_INI" "$before"
  cp "$before" "$WORK/MiSTer.ini.$stamp.bak"
  mister run "[ -f '$REMOTE_BACKUP' ] || cp '$REMOTE_INI' '$REMOTE_BACKUP'"

  echo "==> Commenting [Menu] video_mode for EDID/native mode"
  awk '
    BEGIN { section = "" }
    {
      sub(/\r$/, "", $0)
      if ($0 ~ /^\[/) {
        section = tolower($0)
      }
      if (section == "[menu]" && $0 ~ /^[[:space:]]*video_mode[[:space:]]*=/) {
        print ";" $0 " ; MiSTer MagiK EDID/native video-mode probe"
      } else {
        print
      }
    }
  ' "$before" >"$after"
  mister put "$after" "$REMOTE_INI"
}

restore_mode() {
  mkdir -p "$WORK"
  mister run "rm -f '$REMOTE_BENCH_REQUEST'; kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true"
  local backup
  backup="$(latest_local_backup || true)"
  if mister run "test -f '$REMOTE_BACKUP'" >/dev/null 2>&1; then
    echo "==> Restoring persistent remote backup $REMOTE_BACKUP"
    mister run "cp '$REMOTE_BACKUP' '$REMOTE_INI'"
  elif [[ -n "$backup" ]]; then
    echo "==> Restoring latest local backup $backup"
    mister put "$backup" "$REMOTE_INI"
  else
    echo "==> No backup found to restore" >&2
    mister run "test -f '$REMOTE_BACKUP'"
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

extract_fb_size_from_log() {
  local log="$1"
  sed -n 's/.*fb0=\([0-9][0-9]*\)x\([0-9][0-9]*\).*/\1 \2/p' "$log" | head -1
}

capture_current_fb_png() {
  local dir="$1"
  local log="$2"
  local raw="$dir/fb0.raw"
  local png="$dir/fb0.png"
  local size w h
  size="$(extract_fb_size_from_log "$log")"
  if [[ -z "$size" ]]; then
    echo "capture warning: could not detect fb size from $log" >&2
    return 1
  fi
  read -r w h <<<"$size"
  echo "==> Capturing framebuffer PNG ${w}x${h}"
  mister run "dd if=/dev/fb0 of=/tmp/mister-mode-sweep-fb.raw bs=1M count=32 2>/dev/null || true; wc -c /tmp/mister-mode-sweep-fb.raw"
  mister get /tmp/mister-mode-sweep-fb.raw "$raw"
  mister raw-to-png "$raw" "$w" "$h" "$png"
  echo "$png" >"$dir/fb0.png.path"
}

sweep_mode() {
  local label="${1:-}"
  local scene="${2:-static_ui}"
  [[ -n "$label" ]] || {
    echo "sweep-mode needs a mode label; run sweep-list" >&2
    exit 2
  }
  safe_scene "$scene" || {
    echo "Unsupported scene for sweep-mode: $scene" >&2
    exit 2
  }

  local mode safe_label stamp dir
  mode="$(mode_value_for_label "$label")"
  safe_label="${label//[^A-Za-z0-9_.-]/_}"
  stamp="$(date -u +%Y%m%dT%H%M%SZ)"
  dir="$WORK/sweeps/${stamp}-${safe_label}-${scene}"
  mkdir -p "$dir"

  echo "==> Sweep mode label=$label mode=$mode scene=$scene"
  printf 'label=%s\nmode=%s\nscene=%s\n' "$label" "$mode" "$scene" >"$dir/run.env"
  if [[ "$mode" == "auto" ]]; then
    write_auto_no_reboot "$dir"
  else
    write_mode_no_reboot "$mode" "$dir"
  fi
  write_bench_request "$scene" 0

  echo "==> Rebooting into $mode and launching $scene through MiSTer_MagiK"
  mister reboot-wait
  mister run "for i in \$(seq 1 30); do test -f '/tmp/mister-magik-bench-$scene.log' && break; sleep 1; done; sleep 6"

  mister get "/tmp/mister-magik-bench-$scene.log" "$dir/$scene.log" || true
  mister get /tmp/mister-magik/main-status.json "$dir/main-status.json" || true
  mister run "cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true" >"$dir/fb-mode.txt" || true

  if [[ -f "$dir/$scene.log" ]]; then
    capture_current_fb_png "$dir" "$dir/$scene.log" || true
    sed -n '1,40p' "$dir/$scene.log" >"$dir/log-head.txt"
    {
      echo "label=$label"
      echo "mode=$mode"
      echo "scene=$scene"
      echo "log=$dir/$scene.log"
      [[ -f "$dir/fb0.png.path" ]] && echo "png=$(cat "$dir/fb0.png.path")"
      echo "fb_mode=$(cat "$dir/fb-mode.txt" 2>/dev/null)"
      grep -m1 '^slint-scale=' "$dir/$scene.log" || true
      grep -m1 '^display-config:' "$dir/$scene.log" || true
      if [[ "$label" == "high" || "$label" == "1440p" || "$label" == "2560x1440" ]]; then
        echo "note=high preset is optional/display-dependent; compare detected fb_mode and visual output against stock MiSTer"
      fi
      grep 'fps ~' "$dir/$scene.log" | tail -3 || true
    } | tee "$dir/summary.txt"
  fi

  echo "==> Visual checkpoint is live on HDMI."
  echo "==> Verify the display, then run another sweep-mode or restore."
  echo "==> Results: $dir"
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

stock_ui_mode_probe() {
  local label="${1:-}"
  [[ -n "$label" ]] || {
    echo "stock-ui-mode needs a mode label; run sweep-list" >&2
    exit 2
  }

  local mode stamp dir before mode_ini stock_ini
  mode="$(mode_value_for_label "$label")"
  stamp="$(date -u +%Y%m%dT%H%M%SZ)"
  dir="$WORK/stock-ui/${stamp}-${label//[^A-Za-z0-9_.-]/_}"
  before="$dir/MiSTer.ini.$stamp.bak"
  mode_ini="$dir/MiSTer.ini.$stamp.mode"
  stock_ini="$dir/MiSTer.ini.$stamp.stock-ui"
  mkdir -p "$dir"

  echo "==> Backing up $REMOTE_INI"
  mister get "$REMOTE_INI" "$before"
  cp "$before" "$WORK/MiSTer.ini.$stamp.bak"
  mister run "[ -f '$REMOTE_BACKUP' ] || cp '$REMOTE_INI' '$REMOTE_BACKUP'"

  echo "==> Writing [Menu] video_mode=$mode and disabling main=MiSTer_MagiK"
  awk -v mode="$mode" -f "$AWK" "$before" >"$mode_ini"
  awk '
    {
      sub(/\r$/, "", $0)
      if ($0 == "main=MiSTer_MagiK") print ";main=MiSTer_MagiK ; stock UI video-mode probe"
      else print
    }
  ' "$mode_ini" >"$stock_ini"
  mister put "$stock_ini" "$REMOTE_INI"

  echo "==> Rebooting into stock MiSTer mode label=$label mode=$mode"
  mister reboot-wait
  echo "==> Check the stock MiSTer OSD. Restore with: scripts/mister-video-mode-test.sh restore"
  echo "==> Files: $dir"
}

stock_ui_auto_probe() {
  local stamp dir before auto_ini stock_ini
  stamp="$(date -u +%Y%m%dT%H%M%SZ)"
  dir="$WORK/stock-ui/${stamp}-auto"
  before="$dir/MiSTer.ini.$stamp.bak"
  auto_ini="$dir/MiSTer.ini.$stamp.auto"
  stock_ini="$dir/MiSTer.ini.$stamp.stock-ui-auto"
  mkdir -p "$dir"

  echo "==> Backing up $REMOTE_INI"
  mister get "$REMOTE_INI" "$before"
  cp "$before" "$WORK/MiSTer.ini.$stamp.bak"
  mister run "[ -f '$REMOTE_BACKUP' ] || cp '$REMOTE_INI' '$REMOTE_BACKUP'"

  echo "==> Commenting [Menu] video_mode for EDID/native stock MiSTer probe"
  awk '
    BEGIN { section = "" }
    {
      sub(/\r$/, "", $0)
      if ($0 ~ /^\[/) {
        section = tolower($0)
      }
      if (section == "[menu]" && $0 ~ /^[[:space:]]*video_mode[[:space:]]*=/) {
        print ";" $0 " ; stock UI EDID/native video-mode probe"
      } else {
        print
      }
    }
  ' "$before" >"$auto_ini"
  awk '
    {
      sub(/\r$/, "", $0)
      if ($0 == "main=MiSTer_MagiK") print ";main=MiSTer_MagiK ; stock UI EDID/native probe"
      else print
    }
  ' "$auto_ini" >"$stock_ini"
  mister put "$stock_ini" "$REMOTE_INI"

  echo "==> Rebooting into stock MiSTer EDID/native mode"
  mister reboot-wait
  echo "==> Check the stock MiSTer OSD. Restore with: scripts/mister-video-mode-test.sh restore"
  echo "==> Files: $dir"
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
  sweep-list)
    sweep_list
    ;;
  sweep-mode)
    shift
    sweep_mode "$@"
    ;;
  stock-ui)
    stock_ui_probe
    ;;
  stock-ui-mode)
    shift
    stock_ui_mode_probe "$@"
    ;;
  stock-ui-auto)
    stock_ui_auto_probe
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
