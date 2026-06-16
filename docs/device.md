# MiSTer Device Notes

This document holds durable MiSTer facts, boot policy, and recovery procedures.
Use `AGENTS.md` for the short checklist.

## Device Facts

- Host: `192.168.1.117`
- SSH: `root` / `1`
- CPU/OS: ARM Cortex-A9 `armv7l`, minimal Linux, glibc 2.31
- Hardware: DE10-Nano, 1 GiB DDR3
- Framebuffer: `/dev/fb0`, driver `MiSTer_fb`
- Graphics stack: no `/dev/dri`, no DRM/KMS
- 32-bit framebuffer byte order: B,G,R,X
- Audio: `/dev/MrAudio` for Linux-side HDMI audio; ALSA exposes only dummy PCM
- Storage: `/media/fat` is exFAT/FUSE, so many small writes are slow

Always use `scripts/mister` for device access. Raw ssh and raw scp are unreliable
in this environment and bypass the wrapper behavior this repo expects.

## Production Boot Policy

Production boot must keep stock MiSTer as the inittab entry:

```text
/etc/inittab -> /media/fat/MiSTer
```

`MiSTer.ini` then selects the MagiK fork:

```ini
[MiSTer]
main=MiSTer_MagiK
direct_video=0

[Menu]
direct_video=0
video_mode=8

[arcade]
direct_video=1

[arcade_vertical]
direct_video=0
video_mode=8
vscale_mode=1
```

Keep `[arcade_vertical]` after `[arcade]`; `MiSTer.ini` is parsed top to bottom
and vertical arcade games match both sections.

Do not set `main=mister-magik-fb`. Slint is not Main and cannot initialize HDMI
before Main's `video_init()`.

Use only the Rust comment-preserving mutators in `scripts/mister` or the project
install/restore scripts for `MiSTer.ini` changes. Do not use ad hoc sed/awk.

## Recovery

Common commands:

```bash
scripts/mister status
scripts/mister reboot-wait
scripts/mister recover
scripts/restore-stock-boot.sh
scripts/install-slint-boot.sh
```

`scripts/mister reboot` and `scripts/mister reboot-wait` use
`mister_magik_reboot` through the Main fork when `MiSTer_MagiK` and
`/dev/MiSTer_cmd` are available, keeping MiSTer MagiK in display ownership until
reset. `reboot-wait` then waits for the down-to-up transition and confirms the
device can run commands. Prefer it over blind sleeps.

Use `scripts/mister reboot --raw` or `scripts/mister reboot-wait --raw` only for
recovery/debugging when the Main fork or command FIFO is broken.

`scripts/restore-stock-boot.sh` restores stock menu boot by repairing inittab,
restoring `MiSTer.ini` from `/media/fat/MiSTer.ini.bak` when appropriate, and
rebooting.

If HDMI is black but SSH works, prefer scripted recovery/reboot over manual
process experiments.

## Framebuffer And HDMI

Writing `/dev/fb0` is not the same as showing pixels on HDMI. The MiSTer HPS
framebuffer has multiple buffers, and the FPGA scans whichever buffer is
selected by the framebuffer route command. At the stock menu, `/dev/fb0` can be
correct while HDMI still shows another buffer.

The launcher path relies on Rust-owned framebuffer setup:

- Set the Linux framebuffer mode.
- Clear/paint the intended frame.
- Route buffer 0 with the FPGA `SET_FBUF` command.
- Keep the route alive while the launcher runs.

Debugging tip: framebuffer dumps are useful only while the UI is running. After
exit, fbcon can redraw the login console into `/dev/fb0`.

## Process And Input Gotchas

- Busybox has no `pkill`; use `pidof` and `kill` through scripts.
- Do not SIGSTOP MiSTer for launcher development. A stopped Main can keep evdev
  grabs and leave the stock OSD visible.
- Do not leave a paused Main process after experiments; reboot if unsure.
- `ui` and `fb` paths put the VT into graphics mode so fbcon does not draw over
  the launcher, and restore text mode on exit.
- Missing libinput quirks DB warnings are expected on the MiSTer. The Linux js
  input path is the current working input path.

## Display Modes

Use preset IDs for standard HDMI modes:

- `0` for 720p
- `8` for 1080p
- `6` for 640x480

Avoid shorthand calculated modes such as `1280,720,60` for serious conclusions;
they can synthesize timings that behave badly on the current display.

CRT/direct-video menu timings are a separate smoke-test path. Use
`scripts/mister-video-mode-test.sh` and restore the persistent INI backup after
direct-video runs.

## Audio

Linux-side HDMI audio is written through `/dev/MrAudio`. The `audio-tone`
subcommand exists as a standalone probe. Do not expect ALSA to expose real HDMI
audio on this device.
