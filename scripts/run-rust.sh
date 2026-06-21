#!/usr/bin/env bash
# Run the already-deployed Rust frontend on the MiSTer.
#
# This is intentionally separate from deploy-rust.sh: use it when the binary on
# /media/fat is already good and you only need to restart a scene.
#
#   scripts/run-rust.sh                  # restart supervised launcher, forever
#   scripts/run-rust.sh launcher 0       # restart supervised launcher, forever
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REMOTE="/media/fat/mister-magik/mister-magik-fb"
SCENE="${1:-launcher}"
SECS="${2:-0}"
LOG="/tmp/mister-magik-${SCENE}.log"
REMOTE_SCENE="$SCENE"
EXTRA_ENV=""

case "$SCENE" in
  launcher) ;;
  arcade|arcade-effects)
    echo "The direct arcade scene was removed. Use scripts/profile-preview-scroll.sh for supervised real Arcade benchmarks." >&2
    exit 2
    ;;
  demo|full_motion|static_ui|local_motion|controller_test|video_playback|solid_fill|dirty_band) ;;
  camera-effects|sprite-effects|text-effects|raster-effects|transition-effects)
    echo "'$SCENE' is an experimental effect scene, not a production UI scene." >&2
    echo "Use scripts/experiments/ and deploy with scripts/deploy-rust.sh --experiments." >&2
    exit 2
    ;;
  -h|--help)
    sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
    ;;
  *)
    echo "Unknown scene '$SCENE'. Run 'scripts/run-rust.sh --help' for examples." >&2
    exit 2
    ;;
esac

if [[ "$SCENE" == "launcher" ]]; then
  echo "==> Restarting Main-supervised launcher"
  "$HERE/scripts/mister" run "rm -f /media/fat/mister-magik/launcher.env; if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd"
  exit 0
fi

echo "==> Starting deployed $REMOTE ui $REMOTE_SCENE $SECS"
if [ -n "$EXTRA_ENV" ]; then
  echo "==> Extra env: $EXTRA_ENV"
fi
echo "==> Log: $LOG"
"$HERE/scripts/mister" run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
test -x '$REMOTE' || chmod +x '$REMOTE'
$EXTRA_ENV '$REMOTE' ui '$REMOTE_SCENE' '$SECS' >'$LOG' 2>&1 &
echo ui_pid=\$!
sleep 0.4
sed -n '1,80p' '$LOG' 2>/dev/null || true
"
