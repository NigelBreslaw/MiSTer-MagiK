#!/bin/sh
# Boot into Slint after stock MiSTer brings up HDMI (video_init).
# MiSTer is stopped before Slint runs so we own SPI, HDMI, and gamepad input.
# Games launch by briefly spawning MiSTer + fifo load_core (see launcher.rs).
# Do NOT set main= to mister-magic-fb — MiSTer skips video_init before that exec.
set -e
MISTER=/media/fat/MiSTer
LAUNCHER=/media/fat/mister-magic/mister-magic-fb
LOG=/tmp/mister-magic-boot.log
MENU_PNG=/media/fat/menu.png
MENU_HIDE=/media/fat/mister-magic/.menu.png.boot-hide

exec >>"$LOG" 2>&1
echo "=== boot.sh $(date) ==="

restore_menu() {
    if [ -f "$MENU_HIDE" ]; then
        mv "$MENU_HIDE" "$MENU_PNG" 2>/dev/null || true
    fi
}
trap restore_menu EXIT INT TERM

# Hide Yoshi wallpaper while stock MiSTer runs video_init (draw_checkers instead).
if [ -f "$MENU_PNG" ] && [ ! -f "$MENU_HIDE" ]; then
    mv "$MENU_PNG" "$MENU_HIDE"
    echo "menu.png hidden for boot"
fi

# MiSTer must own main= so user_io_init runs video_init() before we take over.
"$MISTER" &
mpid=$!
echo "started MiSTer pid=$mpid"

# Wait for menu core + 1080p fb (fifo optional — we spawn MiSTer again at launch).
ready=0
i=0
while [ "$i" -lt 150 ]; do
    if pidof MiSTer >/dev/null 2>&1; then
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

echo "stopping MiSTer (Slint owns SPI + input)"
kill -9 $(pidof MiSTer) 2>/dev/null || true
sleep 0.5

echo "handoff to Slint launcher"
exec "$LAUNCHER" ui launcher 0
