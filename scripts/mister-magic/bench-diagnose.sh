#!/bin/sh
# On-device benchmark helper — logs every step, flushes immediately.
# Invoked by scripts/bench-diagnose.sh on the host (do not run long jobs via raw SSH).
#
# Usage on device:
#   bench-diagnose.sh visible  full_motion 15
#   bench-diagnose.sh sigstop   static_ui 10
#   bench-diagnose.sh vsync     static_ui 10    # alias for sigstop (timing; HDMI may stay menu)
set -u

REMOTE="${MISTER_MAGIC_FB:-/media/fat/mister-magic/mister-magic-fb}"
MISTER="${MISTER_BIN:-/media/fat/MiSTer}"
LOG="${BENCH_LOG:-/tmp/bench-diagnose.log}"

MODE="${1:-}"
SCENE="${2:-full_motion}"
SECS="${3:-15}"

log() {
	# busybox date: -u may be missing; fall back
	ts=$(date -u +%H:%M:%S 2>/dev/null || date +%H:%M:%S)
	printf '%s %s\n' "$ts" "$*"
	printf '%s %s\n' "$ts" "$*" >>"$LOG"
}

usage() {
	echo "usage: $0 <visible|sigstop|vsync> [scene] [secs]"
	exit 2
}

[ -n "$MODE" ] || usage
case "$MODE" in
visible | sigstop | vsync) ;;
-h | --help) usage ;;
*) usage ;;
esac

[ -x "$REMOTE" ] || {
	log "ERROR: missing $REMOTE"
	exit 1
}

: >"$LOG"
log "=== bench-diagnose mode=$MODE scene=$SCENE secs=$SECS ==="
log "log file: $LOG"

cleanup() {
	if [ -n "${MP_STOPPED:-}" ]; then
		log "CONT MiSTer pid=$MP_STOPPED"
		kill -CONT "$MP_STOPPED" 2>/dev/null || true
	fi
}
trap cleanup EXIT INT TERM

case "$MODE" in
visible)
	log "kill MiSTer (Slint owns SPI + HDMI; you should see the bench on TV)"
	kill -9 $(pidof MiSTer) 2>/dev/null || true
	sleep 0.5
	log "MiSTer pid: $(pidof MiSTer 2>/dev/null || echo none)"
	;;
sigstop | vsync)
	log "start stock MiSTer for menu video timing (HDMI may show menu wallpaper)"
	kill -9 $(pidof mister-magic-fb) 2>/dev/null || true
	kill -9 $(pidof MiSTer) 2>/dev/null || true
	sleep 0.5
	"$MISTER" &
	MP_STOPPED=$!
	log "MiSTer pid=$MP_STOPPED (waiting for 1080p fb)"
	ready=0
	i=0
	while [ "$i" -lt 150 ]; do
		if pidof MiSTer >/dev/null 2>&1; then
			mode=$(cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true)
			case "$mode" in
			*1920*1080*)
				ready=1
				break
				;;
			esac
		fi
		i=$((i + 1))
		usleep 200000 2>/dev/null || sleep 0.2
	done
	log "1080p ready=$ready mode=$(cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || echo '?')"
	log "SIGSTOP MiSTer (menu frozen; bench may be fb0-only on HDMI)"
	kill -STOP "$MP_STOPPED"
	;;
esac

log "running: $REMOTE ui $SCENE $SECS"
log "(fps lines stream below; wait ${SECS}s — this is normal)"
"$REMOTE" ui "$SCENE" "$SECS" 2>&1 | while IFS= read -r line; do
	log "  $line"
done
rc=0
log "ui finished"

log "=== done mode=$MODE scene=$SCENE ==="
exit "$rc"
