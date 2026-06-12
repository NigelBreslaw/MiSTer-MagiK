#!/usr/bin/env bash
# Run the already-deployed Rust frontend on the MiSTer.
#
# This is intentionally separate from deploy-rust.sh: use it when the binary on
# /media/fat is already good and you only need to restart a scene.
#
#   scripts/run-rust.sh                  # real arcade screen, forever
#   scripts/run-rust.sh launcher 0       # full launcher, forever
#   scripts/run-rust.sh arcade-effects 0 # arcade screen with left/right effect picker
#   scripts/run-rust.sh camera-effects 0 # full-screen classic camera effect picker
#   scripts/run-rust.sh sprite-effects 0 # full-screen classic sprite effect picker
#   scripts/run-rust.sh console_scroll 15
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REMOTE="/media/fat/mister-magik/mister-magik-fb"
SCENE="${1:-arcade}"
SECS="${2:-0}"
LOG="/tmp/mister-magik-${SCENE}.log"
REMOTE_SCENE="$SCENE"
EXTRA_ENV=""

case "$SCENE" in
  demo|full_motion|static_ui|local_motion|console_scroll|launcher|controller_test|arcade|blend_velocity|video_playback|solid_fill|dirty_band) ;;
  arcade-effects)
    REMOTE_SCENE="arcade"
    EXTRA_ENV="MISTER_FB_FORMAT=565 MISTER_PREVIEW_BLITTER=raw MISTER_PREVIEW_FORMAT=raw-rgb565 MISTER_PREVIEW_TRANSITION_PICKER=1 MISTER_PREVIEW_TRANSITION_MS=900"
    ;;
  camera-effects)
    EXTRA_ENV="MISTER_FB_FORMAT=565 MISTER_PREVIEW_FORMAT=raw-rgb565 MISTER_CAMERA_EFFECTS_HUD=1"
    ;;
  sprite-effects)
    EXTRA_ENV="MISTER_FB_FORMAT=565 MISTER_PREVIEW_FORMAT=raw-rgb565 MISTER_SPRITE_EFFECTS_HUD=1"
    ;;
  -h|--help)
    sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
    ;;
  *)
    echo "Unknown scene '$SCENE'. Run 'scripts/run-rust.sh --help' for examples." >&2
    exit 2
    ;;
esac

echo "==> Starting deployed $REMOTE ui $REMOTE_SCENE $SECS"
if [ -n "$EXTRA_ENV" ]; then
  echo "==> Extra env: $EXTRA_ENV"
fi
echo "==> Log: $LOG"
"$HERE/scripts/mister" run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
kill -9 \$(pidof MiSTer_MagiK) 2>/dev/null || true
kill -9 \$(pidof MiSTer) 2>/dev/null || true
test -x '$REMOTE' || chmod +x '$REMOTE'
$EXTRA_ENV '$REMOTE' ui '$REMOTE_SCENE' '$SECS' >'$LOG' 2>&1 &
echo ui_pid=\$!
sleep 0.4
sed -n '1,80p' '$LOG' 2>/dev/null || true
"
