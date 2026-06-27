#!/usr/bin/env bash
# Temporarily switch MiSTer.ini [Menu] video_mode for display-mode validation.
#
# Common PR4 flow:
#   scripts/mister-video-mode-test.sh set-960
#   scripts/mister-video-mode-test.sh magik-run launcher 12
#   scripts/mister-video-mode-test.sh restore
#
# Common PR5 flow:
#   scripts/mister-video-mode-test.sh sweep-list
#   scripts/mister-video-mode-test.sh sweep-mode 960 launcher
#   # visually verify, then repeat for the next label
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
WORK="$ROOT/build/mister-video-mode-test"
REMOTE_INI="/media/fat/MiSTer.ini"
REMOTE_BACKUP="/media/fat/MiSTer.ini.magik-mode-test.bak"
REMOTE_BENCH_REQUEST="/media/fat/mister-magik/bench-boot"

usage() {
  cat <<EOF
Usage:
  scripts/mister-video-mode-test.sh set MODE
  scripts/mister-video-mode-test.sh set-960
  scripts/mister-video-mode-test.sh magik-run [SCENE] [SECS]
  scripts/mister-video-mode-test.sh magik-sweep [SECS]
  scripts/mister-video-mode-test.sh sweep-list
  scripts/mister-video-mode-test.sh sweep-mode LABEL [SCENE]
  scripts/mister-video-mode-test.sh stock-ui-mode LABEL
  scripts/mister-video-mode-test.sh stock-ui-auto
  scripts/mister-video-mode-test.sh crt-list
  scripts/mister-video-mode-test.sh crt-smoke LABEL [stock|magik]
  scripts/mister-video-mode-test.sh stock-ui
  scripts/mister-video-mode-test.sh restore

Examples:
  scripts/mister-video-mode-test.sh set-960
  scripts/mister-video-mode-test.sh magik-run launcher 12
  scripts/mister-video-mode-test.sh magik-sweep 12
  scripts/mister-video-mode-test.sh sweep-list
  scripts/mister-video-mode-test.sh sweep-mode 720p launcher
  scripts/mister-video-mode-test.sh stock-ui-mode high
  scripts/mister-video-mode-test.sh stock-ui-auto
  scripts/mister-video-mode-test.sh crt-list
  scripts/mister-video-mode-test.sh crt-smoke ntsc15 stock
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
  scripts/mister-video-mode-test.sh sweep-mode auto launcher
  scripts/mister-video-mode-test.sh sweep-mode low launcher
  scripts/mister-video-mode-test.sh sweep-mode 960 launcher
  scripts/mister-video-mode-test.sh sweep-mode 720p launcher

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

crt_params_for_label() {
  case "$1" in
    ntsc15|ntsc|240p)
      printf '1\t0\t0\tNTSC 15 kHz direct-video timing (640x240)\n'
      ;;
    ntsc31|ntsc-vga|480p)
      printf '1\t0\t1\tNTSC 31 kHz direct-video timing (640x480)\n'
      ;;
    pal15|pal|288p)
      printf '1\t1\t0\tPAL 15 kHz direct-video timing (640x288)\n'
      ;;
    pal31|pal-vga|576p)
      printf '1\t1\t1\tPAL 31 kHz direct-video timing (640x576)\n'
      ;;
    direct-auto|dac-auto)
      printf '2\t0\t0\tDirect-video auto-detect for known HDMI DACs\n'
      ;;
    *)
      echo "Unknown CRT/direct-video label: $1" >&2
      return 1
      ;;
  esac
}

crt_list() {
  cat <<EOF
CRT/direct-video smoke labels:
  ntsc15      -> direct_video=1 menu_pal=0 forced_scandoubler=0  (640x240, 15 kHz)
  ntsc31      -> direct_video=1 menu_pal=0 forced_scandoubler=1  (640x480, 31 kHz)
  pal15       -> direct_video=1 menu_pal=1 forced_scandoubler=0  (640x288, 15 kHz)
  pal31       -> direct_video=1 menu_pal=1 forced_scandoubler=1  (640x576, 31 kHz)
  direct-auto -> direct_video=2 menu_pal=0 forced_scandoubler=0  (known HDMI DAC auto-detect)

Run only when the matching analog/direct-video output path is physically
connected, or when you are intentionally testing stock MiSTer failure/recovery:
  scripts/mister-video-mode-test.sh crt-smoke ntsc15 stock
  scripts/mister-video-mode-test.sh crt-smoke ntsc31 magik

Each smoke run backs up MiSTer.ini, writes only the MiSTer direct-video keys,
reboots, records the resulting INI/fb/status logs, and leaves the display live
for visual verification. Restore with:
  scripts/mister-video-mode-test.sh restore
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
  mister ini-edit-local menu-mode "$mode" "$before" "$after"
  mister put "$after" "$REMOTE_INI"

  echo "==> Rebooting into video_mode=$mode"
  mister reboot-wait
  echo "==> Mode set; run a scene with: scripts/mister-video-mode-test.sh magik-run launcher 12"
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
  mister ini-edit-local menu-mode "$mode" "$before" "$after"
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
  mister ini-edit-local menu-auto "$before" "$after"
  mister put "$after" "$REMOTE_INI"
}

write_crt_no_reboot() {
  local label="$1"
  local out_dir="$2"
  mkdir -p "$WORK" "$out_dir"

  local direct_video menu_pal forced_scandoubler description
  IFS=$'\t' read -r direct_video menu_pal forced_scandoubler description < <(crt_params_for_label "$label")

  local stamp
  stamp="$(date -u +%Y%m%dT%H%M%SZ)"
  local before="$out_dir/MiSTer.ini.$stamp.bak"
  local after="$out_dir/MiSTer.ini.$stamp.crt"

  echo "==> Backing up $REMOTE_INI"
  mister get "$REMOTE_INI" "$before"
  cp "$before" "$WORK/MiSTer.ini.$stamp.bak"
  mister run "[ -f '$REMOTE_BACKUP' ] || cp '$REMOTE_INI' '$REMOTE_BACKUP'"

  echo "==> Writing CRT/direct-video settings: $description"
  mister ini-edit-local crt "$direct_video" "$menu_pal" "$forced_scandoubler" "$before" "$after"
  mister put "$after" "$REMOTE_INI"
  printf 'label=%s\ndirect_video=%s\nmenu_pal=%s\nforced_scandoubler=%s\ndescription=%s\n' \
    "$label" "$direct_video" "$menu_pal" "$forced_scandoubler" "$description" >"$out_dir/crt.env"
}

restore_mode() {
  mkdir -p "$WORK"
  mister run "rm -f '$REMOTE_BENCH_REQUEST'"
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
  echo "==> Re-applying MiSTer MagiK boot/video INI policy"
  mister ini-repair-boot
  mister ini-repair-arcade-video
  echo "==> Rebooting after restore"
  if ! mister reboot-wait; then
    echo "==> Supervised reboot did not complete; falling back to raw reboot recovery" >&2
    mister reboot-wait --raw
  fi
}

safe_scene() {
  case "$1" in
    launcher|controller_test|video_playback) return 0 ;;
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
  mister run "mkdir -p /media/fat/mister-magik; printf '%s %s\n' '$scene' '$secs' > '$REMOTE_BENCH_REQUEST'; rm -f '/tmp/mister-magik-bench-$scene.log'"
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
  local scene="${1:-launcher}"
  local secs="${2:-12}"
  echo "==> Running through MiSTer MagiK bench boot scene=$scene secs=$secs"
  write_bench_request "$scene" "$secs"
  mister reboot-wait
  show_bench_log "$scene" "$secs"
}

magik_sweep() {
  local secs="${1:-12}"
  local scenes=(launcher controller_test)
  for scene in "${scenes[@]}"; do
    echo "=== MiSTer MagiK sweep scene=$scene secs=$secs ==="
    write_bench_request "$scene" "$secs"
    mister reboot-wait
    show_bench_log "$scene" "$secs"
  done
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
  local scene="${2:-launcher}"
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
  mister ini-edit-local comment-main "$before" "$after"
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
  mister ini-edit-local menu-mode "$mode" "$before" "$mode_ini"
  mister ini-edit-local comment-main "$mode_ini" "$stock_ini"
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
  mister ini-edit-local menu-auto "$before" "$auto_ini"
  mister ini-edit-local comment-main "$auto_ini" "$stock_ini"
  mister put "$stock_ini" "$REMOTE_INI"

  echo "==> Rebooting into stock MiSTer EDID/native mode"
  mister reboot-wait
  echo "==> Check the stock MiSTer OSD. Restore with: scripts/mister-video-mode-test.sh restore"
  echo "==> Files: $dir"
}

comment_main_for_probe() {
  local in_file="$1"
  local out_file="$2"
  mister ini-edit-local comment-main "$in_file" "$out_file"
}

crt_smoke() {
  local label="${1:-}"
  local owner="${2:-stock}"
  [[ -n "$label" ]] || {
    echo "crt-smoke needs a label; run crt-list" >&2
    exit 2
  }
  case "$owner" in
    stock|magik) ;;
    *)
      echo "crt-smoke owner must be stock or magik" >&2
      exit 2
      ;;
  esac

  local safe_label stamp dir stock_ini
  safe_label="${label//[^A-Za-z0-9_.-]/_}"
  stamp="$(date -u +%Y%m%dT%H%M%SZ)"
  dir="$WORK/crt/${stamp}-${safe_label}-${owner}"
  mkdir -p "$dir"

  echo "==> CRT/direct-video smoke label=$label owner=$owner"
  write_crt_no_reboot "$label" "$dir"

  if [[ "$owner" == "stock" ]]; then
    stock_ini="$dir/MiSTer.ini.$stamp.stock-crt"
    comment_main_for_probe "$dir"/MiSTer.ini.*.crt "$stock_ini"
    mister put "$stock_ini" "$REMOTE_INI"
  else
    write_bench_request launcher 0
  fi

  echo "==> Rebooting into CRT/direct-video smoke path"
  mister reboot-wait
  if [[ "$owner" == "magik" ]]; then
    mister run "for i in \$(seq 1 30); do test -f '/tmp/mister-magik-bench-launcher.log' && break; sleep 1; done; sleep 6"
    mister get /tmp/mister-magik-bench-launcher.log "$dir/launcher.log" || true
    mister get /tmp/mister-magik/main-status.json "$dir/main-status.json" || true
  fi
  mister run "awk 'BEGIN{s=\"global\"} /^\\[/ {s=\$0} /^[;[:space:]]*(main|video_mode|direct_video|menu_pal|forced_scandoubler|fb_size|fb_terminal)[[:space:]]*=/ {print s \" \" NR \":\" \$0}' '$REMOTE_INI'; echo -n 'fb_mode='; cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true; echo; cat /tmp/mister-magik/main-status.json 2>/dev/null || true" \
    | tee "$dir/device-summary.txt"

  echo "==> CRT/direct-video checkpoint is live."
  echo "==> Verify the connected output if present, then restore with: scripts/mister-video-mode-test.sh restore"
  echo "==> Results: $dir"
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
  crt-list)
    crt_list
    ;;
  crt-smoke)
    shift
    crt_smoke "$@"
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
