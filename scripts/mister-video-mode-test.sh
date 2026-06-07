#!/usr/bin/env bash
# Temporarily switch MiSTer.ini [Menu] video_mode for display-mode validation.
#
# Common PR4 flow:
#   scripts/mister-video-mode-test.sh set-960
#   scripts/mister-video-mode-test.sh stock-ui
#   scripts/mister-video-mode-test.sh pattern 0 normal
#   scripts/mister-video-mode-test.sh run static_ui 0
#   scripts/mister-video-mode-test.sh restore
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$ROOT/scripts/mister"
AWK="$ROOT/scripts/mister-magik/set-menu-video-mode.awk"
WORK="$ROOT/build/mister-video-mode-test"
REMOTE_INI="/media/fat/MiSTer.ini"
REMOTE_BACKUP="/media/fat/MiSTer.ini.magik-mode-test.bak"
REMOTE_BIN="/media/fat/mister-magik/mister-magik-fb"

usage() {
  cat <<EOF
Usage:
  scripts/mister-video-mode-test.sh set MODE
  scripts/mister-video-mode-test.sh set-960
  scripts/mister-video-mode-test.sh stock-ui
  scripts/mister-video-mode-test.sh pattern [SECS] [normal|direct|none]
  scripts/mister-video-mode-test.sh run [SCENE] [SECS]
  scripts/mister-video-mode-test.sh restore

Examples:
  scripts/mister-video-mode-test.sh set-960
  scripts/mister-video-mode-test.sh stock-ui
  scripts/mister-video-mode-test.sh pattern 0 normal
  scripts/mister-video-mode-test.sh run static_ui 0
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
  echo "==> Mode set; run a scene with: scripts/mister-video-mode-test.sh run static_ui 0"
}

restore_mode() {
  mkdir -p "$WORK"
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
