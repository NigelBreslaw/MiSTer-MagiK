#!/bin/sh
# Legacy boot.sh handoff. Production uses stock `/media/fat/MiSTer` in inittab
# plus `[MiSTer] main=MiSTer_MagiK`, matching Zaparoo and preserving update_all.
# This file is kept only for historical recovery experiments.
# Boot into Slint after MiSTer_MagiK brings up HDMI (video_init).
# MiSTer_MagiK is stopped before Slint runs so we own SPI, HDMI, and gamepad input.
# Games launch by briefly spawning MiSTer + fifo load_core (see launcher.rs).
# Do NOT set main= to mister-magik-fb — MiSTer skips video_init before that exec.
set -e
MISTER=/media/fat/MiSTer_MagiK
LAUNCHER=/media/fat/mister-magik/mister-magik-fb
LOG=/tmp/mister-magik-boot.log
MENU_PNG=/media/fat/menu.png
MENU_HIDE=/media/fat/mister-magik/.menu.png.boot-hide
STOCK_MARKER=/tmp/mister-magik-stock-menu

exec >>"$LOG" 2>&1
echo "=== boot.sh $(date) ==="

restore_menu() {
    if [ -f "$MENU_HIDE" ]; then
        mv "$MENU_HIDE" "$MENU_PNG" 2>/dev/null || true
    fi
}
trap restore_menu EXIT INT TERM

if [ -f "$STOCK_MARKER" ]; then
    echo "stock MiSTer marker present; staying out of Magik until reboot"
    restore_menu
    if pidof MiSTer >/dev/null 2>&1; then
        echo "MiSTer_MagiK already running; waiting without relaunching Magik"
        while pidof MiSTer_MagiK >/dev/null 2>&1 || pidof MiSTer >/dev/null 2>&1; do
            sleep 60
        done
        exit 0
    fi
    exec "$MISTER"
fi

# Hide Yoshi wallpaper while stock MiSTer runs video_init (draw_checkers instead).
if [ -f "$MENU_PNG" ] && [ ! -f "$MENU_HIDE" ]; then
    mv "$MENU_PNG" "$MENU_HIDE"
    echo "menu.png hidden for boot"
fi

# MiSTer_MagiK must initialize Main/video before we take over.
"$MISTER" &
mpid=$!
echo "started MiSTer_MagiK pid=$mpid"

# Wait for menu core + 1080p fb (fifo optional — we spawn MiSTer again at launch).
ready=0
i=0
while [ "$i" -lt 150 ]; do
    if pidof MiSTer_MagiK >/dev/null 2>&1 || pidof MiSTer >/dev/null 2>&1; then
        mode=$(cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null || true)
        case "$mode" in
            *1920*1080*) ready=1; break ;;
        esac
    fi
    i=$((i + 1))
    usleep 200000 2>/dev/null || sleep 1
done

if [ "$ready" != 1 ]; then
    echo "timeout waiting for 1080p fb (mode=$(cat /sys/module/MiSTer_fb/parameters/mode 2>/dev/null)); continuing anyway"
fi

echo "stopping stock/fork MiSTer owners (Slint owns SPI + input)"
kill -9 $(pidof MiSTer_MagiK) 2>/dev/null || true
kill -9 $(pidof MiSTer) 2>/dev/null || true
sleep 0.5

echo "handoff to Slint launcher"
exec "$LAUNCHER" ui launcher 0
