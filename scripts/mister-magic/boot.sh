#!/bin/sh
# Boot into Slint after stock MiSTer brings up HDMI (video_init).
# MiSTer must exit so Slint owns evdev (MiSTer keeps grabs while SIGSTOPped).
# Do NOT set main= to mister-magic-fb — MiSTer skips video_init before that exec.
set -e
MISTER=/media/fat/MiSTer
LAUNCHER=/media/fat/mister-magic/mister-magic-fb
LOG=/tmp/mister-magic-boot.log

exec >>"$LOG" 2>&1
echo "=== boot.sh $(date) ==="

# MiSTer must own main= so user_io_init runs video_init() before we take over.
"$MISTER" &
mpid=$!
echo "started MiSTer pid=$mpid"

# Wait for menu core + 1080p fb (video_init completed).
ready=0
i=0
while [ "$i" -lt 45 ]; do
    if pidof MiSTer >/dev/null 2>&1; then
        mode=$(cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true)
        case "$mode" in
            *1920*1080*) ready=1; break ;;
        esac
    fi
    i=$((i + 1))
    sleep 1
done

if [ "$ready" != 1 ]; then
    echo "timeout waiting for 1080p fb (mode=$(cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null)); continuing anyway"
    sleep 2
fi

killall MiSTer 2>/dev/null || true
rm -f /tmp/mister-magic-mister.pid
sleep 1
echo "handoff to Slint launcher"
exec "$LAUNCHER" ui launcher 0
