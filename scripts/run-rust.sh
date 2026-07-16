#!/usr/bin/env bash
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

# Run the already-deployed Rust frontend on the MiSTer.
#
# This is intentionally separate from deploy-rust.sh: use it when the binary on
# /media/fat is already good and you only need to restart a scene.
#
#   scripts/run-rust.sh                  # restart supervised launcher, forever
#   scripts/run-rust.sh launcher 0       # restart supervised launcher, forever
#   scripts/run-rust.sh tear_pattern 10  # run the visual tearing test scene
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MISTER="$HERE/scripts/mister"
source "$HERE/scripts/lib/magik-layout.sh"
magik_layout_select dev
source "$HERE/scripts/lib/mister-supervision-lib.sh"
REMOTE="$MISTER_MAGIK_BIN"
SCENE="${1:-launcher}"
SECS="${2:-0}"
LOG="/tmp/mister-magik-${SCENE}.log"
REMOTE_SCENE="$SCENE"
EXTRA_ENV=""
REMOTE_ENV="$MISTER_MAGIK_LAUNCHER_ENV"

usage() {
  cat <<'EOF'
Usage:
  scripts/run-rust.sh [launcher] [0]
  scripts/run-rust.sh controller_test [SECS]
  scripts/run-rust.sh tear_pattern [SECS]
  scripts/run-rust.sh video_playback [SECS]

Runs the already-deployed Rust frontend on the MiSTer.
EOF
}

case "$SCENE" in
  launcher) ;;
  arcade|arcade-effects)
    echo "The direct arcade scene was removed. Use scripts/profile-preview-scroll.sh for supervised real Arcade benchmarks." >&2
    exit 2
    ;;
  controller_test|tear_pattern|video_playback) ;;
  camera-effects|sprite-effects|text-effects|raster-effects|transition-effects)
    echo "'$SCENE' is an experimental effect scene, not a production UI scene." >&2
    echo "Use scripts/experiments/ and deploy with scripts/deploy-rust.sh --experiments." >&2
    exit 2
    ;;
  -h|--help)
    usage
    exit 0
    ;;
  *)
    echo "Unknown scene '$SCENE'. Run 'scripts/run-rust.sh --help' for examples." >&2
    exit 2
    ;;
esac

if [[ "$SCENE" == "launcher" ]]; then
  echo "==> Restarting Main-supervised launcher"
  env_exports=()
  for name in \
    MISTER_PRESENT_BACKEND \
    MISTER_UI_FB_SIZE \
    MISTER_FB_PRESENT_DELAY_US \
    MISTER_LAUNCHER_START_SCREEN \
    MISTER_LAUNCHER_LOCK_SCREEN \
    MISTER_LAUNCHER_BENCH_SCENARIO
  do
    if [[ -n "${!name:-}" ]]; then
      env_exports+=("export $name=$(printf '%q' "${!name}")")
    fi
  done
  if [[ "${#env_exports[@]}" -eq 0 ]]; then
    "$MISTER" run "rm -f '$REMOTE_ENV'; if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd"
  else
    tmp_env="$(mktemp "${TMPDIR:-/tmp}/mister-magik-launcher-env.XXXXXX")"
    printf '%s\n' "${env_exports[@]}" >"$tmp_env"
    "$MISTER" put "$tmp_env" "$REMOTE_ENV" >/dev/null
    rm -f "$tmp_env"
    "$MISTER" run "if [ ! -p /dev/MiSTer_cmd ]; then echo 'missing /dev/MiSTer_cmd'; exit 12; fi; printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd"
  fi
  exit 0
fi

echo "==> Starting deployed $REMOTE ui $REMOTE_SCENE $SECS"
if [ -n "$EXTRA_ENV" ]; then
  echo "==> Extra env: $EXTRA_ENV"
fi
echo "==> Log: $LOG"
mister_suspend_launcher
trap 'mister_restart_launcher >/dev/null 2>&1 || true' EXIT
"$MISTER" run "
set -e
kill -9 \$(pidof mister-magik-fb) 2>/dev/null || true
test -x '$REMOTE' || chmod +x '$REMOTE'
$EXTRA_ENV '$REMOTE' ui '$REMOTE_SCENE' '$SECS' >'$LOG' 2>&1 &
echo ui_pid=\$!
sleep 0.4
sed -n '1,80p' '$LOG' 2>/dev/null || true
"
