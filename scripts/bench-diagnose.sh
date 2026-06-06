#!/usr/bin/env bash
# Run MiSTer UI benchmarks with live, flushed progress (no silent 80s SSH blob).
#
#   MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-diagnose.sh visible full_motion 15
#   scripts/bench-diagnose.sh sigstop static_ui 10   # timing; TV may stay menu wallpaper
#   scripts/bench-diagnose.sh --restore-launcher     # after tests, hand back to Slint boot
#
# Uses MISTER_CMD_TIMEOUT=0 (no limit) and streams stdout line-by-line.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SSH="$HERE/scripts/mister_ssh.py"
DEVICE_SCRIPT="/media/fat/mister-magik/bench-diagnose.sh"
HOST_SCRIPT="$HERE/scripts/mister-magik/bench-diagnose.sh"

export MISTER_IP="${MISTER_IP:-192.168.1.117}"
export MISTER_PASS="${MISTER_PASS:-1}"
export MISTER_CMD_TIMEOUT="${MISTER_CMD_TIMEOUT:-0}"

RESTORE_LAUNCHER=0
MODE=""
SCENE="full_motion"
SECS=15

usage() {
  sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
  echo ""
  echo "Usage: bench-diagnose.sh [--restore-launcher] <visible|sigstop|vsync> [scene] [secs]"
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) usage 0 ;;
    --restore-launcher) RESTORE_LAUNCHER=1; shift ;;
    visible|sigstop|vsync)
      MODE="$1"
      shift
      [[ $# -gt 0 && "$1" != -* ]] && { SCENE="$1"; shift; }
      [[ $# -gt 0 && "$1" != -* ]] && { SECS="$1"; shift; }
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage 1
      ;;
  esac
done

mister() {
  uv run python "$SSH" "$@"
}

if [[ "$RESTORE_LAUNCHER" -eq 1 && -z "$MODE" ]]; then
  echo "==> restore Slint launcher (kill MiSTer, exec boot handoff path)"
  mister run "
kill -9 \$(pidof MiSTer) 2>/dev/null || true
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
exec /media/fat/mister-magik/mister-magik-fb ui launcher 0
" --stream
  exit 0
fi

[[ -n "$MODE" ]] || usage 1

echo "==> deploy bench-diagnose.sh to device"
mister put "$HOST_SCRIPT" "$DEVICE_SCRIPT"
mister run "chmod +x $DEVICE_SCRIPT" --stream

echo "==> run on device (mode=$MODE scene=$SCENE secs=$SECS, timeout=none, streaming)"
echo "    Tip: visible = bench on HDMI. sigstop/vsync = timing test; TV may show Yoshi menu."
echo ""
mister run "$DEVICE_SCRIPT $MODE $SCENE $SECS" --stream

echo ""
echo "==> pull log"
LOG_LOCAL="$HERE/build/bench-diagnose-$(date -u +%Y%m%dT%H%M%SZ).log"
mkdir -p "$HERE/build"
if mister get /tmp/bench-diagnose.log "$LOG_LOCAL" 2>/dev/null; then
  echo "    saved $LOG_LOCAL"
else
  echo "    (no /tmp/bench-diagnose.log on device)"
fi
