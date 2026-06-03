#!/bin/bash
# Zaparoo frontend shim — Option A piggyback (see AGENTS.md §7).
#
# The Zaparoo fork binary (MiSTer_Zaparoo) launches whatever file is at
# /media/fat/zaparoo/frontend on tty2 AFTER it has loaded its menu core and
# enabled the HPS framebuffer scan-out (video_fb_enable(1) on /dev/fb0). By
# installing this shim there (with the real Qt frontend renamed to
# frontend.real), Zaparoo does all the framebuffer/VT plumbing and then hands
# off to our Slint app.
#
# It may be invoked with "--crt" if CRT mode is persisted; we ignore args and
# always take the HDMI path. MISTER_MAGIC_NO_VMODE=1 stops run-mister.sh from
# re-running vmode (Zaparoo already configured the video pipeline).
#
# Revert: mv /media/fat/zaparoo/frontend.real /media/fat/zaparoo/frontend
export MISTER_MAGIC_NO_VMODE=1
# Performance debugging: log FPS + draw an on-screen FPS overlay so the
# animation's smoothness can be inspected directly on HDMI. Remove once tuned.
export MISTER_MAGIC_PERF=1
exec /media/fat/mister-magic/run-mister.sh
