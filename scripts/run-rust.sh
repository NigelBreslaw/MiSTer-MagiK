#!/usr/bin/env bash
# Run the already-deployed Rust frontend on the MiSTer.
#
# This is intentionally separate from deploy-rust.sh: use it when the binary on
# /media/fat is already good and you only need to restart a scene.
#
#   scripts/run-rust.sh                  # real arcade screen, forever
#   scripts/run-rust.sh launcher 0       # full launcher, forever
#   scripts/run-rust.sh console_scroll 15
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REMOTE="/media/fat/mister-magik/mister-magik-fb"
SCENE="${1:-arcade}"
SECS="${2:-0}"
LOG="/tmp/mister-magik-${SCENE}.log"

case "$SCENE" in
  demo|full_motion|static_ui|local_motion|console_scroll|launcher|controller_test|arcade|blend_velocity|video_playback|solid_fill|dirty_band) ;;
  -h|--help)
    sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
    ;;
  *)
    echo "Unknown scene '$SCENE'. Run 'scripts/run-rust.sh --help' for examples." >&2
    exit 2
    ;;
esac

echo "==> Starting deployed $REMOTE ui $SCENE $SECS"
echo "==> Log: $LOG"
"$HERE/scripts/mister" run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
kill -9 \$(pidof MiSTer_MagiK) 2>/dev/null || true
kill -9 \$(pidof MiSTer) 2>/dev/null || true
test -x '$REMOTE' || chmod +x '$REMOTE'
'$REMOTE' ui '$SCENE' '$SECS' >'$LOG' 2>&1 &
echo ui_pid=\$!
sleep 0.4
sed -n '1,80p' '$LOG' 2>/dev/null || true
"
