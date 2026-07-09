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
install/restore scripts for `MiSTer.ini` changes. Stock inittab repair is
centralized in `scripts/mister inittab-ensure-stock`. Do not use ad hoc sed/awk.

## Recovery

Common commands:

```bash
scripts/mister status
scripts/mister agent status
scripts/mister reboot-wait
scripts/mister recover
scripts/restore-stock-boot.sh
scripts/install-slint-boot.sh
```

`scripts/mister reboot` and `scripts/mister reboot-wait` default to the
supervised MagiK reboot path. That sends `mister_magik_reboot` through
`/dev/MiSTer_cmd`, keeps MagiK display ownership until reset, and the Main fork
then asks Linux to reboot through `/sbin/reboot` so Ethernet gets the normal
kernel shutdown path. `reboot-wait` waits for the down-to-up transition and
confirms the device can run commands. Prefer it over blind sleeps.

Use `scripts/mister reboot --raw` or `scripts/mister reboot-wait --raw` only for
fallback recovery or for testing the detached Linux reboot path without MagiK
visual lockdown.

Use `--direct-reset` for fast development reboots only when all writes are
known to be complete and synced. It enters the same MagiK reboot lockdown, then
calls Main's reset-manager path instead of `/sbin/reboot`, avoiding the slow
BusyBox shutdown path. Keep the supervised default for settings changes,
INI/video-mode edits, release gates, Ethernet soaks, and any unknown write
state. Use `--direct-reset-no-sync` only for explicit attended experiments.

For shutdown timing, install the reversible deep trace with:

```bash
scripts/mister-shutdown-trace.sh install-deep
scripts/mister agent boot-profile 5 --timeout 60 --fail-on-timeout
scripts/mister-shutdown-trace.sh log
scripts/mister-shutdown-trace.sh remove
```

To isolate user shutdown hooks, use the separate reversible
`scripts/mister-shutdown-trace.sh bypass-s99user-install` and
`bypass-s99user-remove` experiment. Summarize collected rows and pulled logs
with `scripts/reboot-shutdown-summary.py`.

To return from an active game core without a Linux reboot, send Main's generic
core command instead of the MagiK supervisor restart command:

```bash
scripts/mister run "printf 'load_core menu.rbf\n' > /dev/MiSTer_cmd"
```

`mister_magik_restart_launcher` is for the menu/supervised-launcher state. Once
Main has handed off to a game, `load_core menu.rbf` exits the active core; the
MagiK menu path then starts `mister-magik-fb` again.

The standalone `mister-magik-agent` may be installed as
`/etc/init.d/S00magik-agent`. It provides the early Ethernet setup path and a
token-protected TCP control port on `7498`; see `docs/magik-agent.md`. TCP
`7497` is reserved for Zaparoo Core. If the agent strands boot networking,
remove `/etc/init.d/S00magik-agent` from the SD card. If needed, restore the
parked legacy FastNet script by renaming
`/etc/init.d/disabled-S00fastnet.magik-agent` back to
`/etc/init.d/S00fastnet`.

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

The experimental vblank-latched Menu RBF is built only by manually starting the
`FPGA Vblank Latch RBF` GitHub Actions workflow. Do not run this workflow on
every push or pull request; Quartus builds are heavyweight and should be kicked
off only when a new shared RBF artifact is actually needed. From a checked-out
repo with GitHub CLI auth:

```bash
gh workflow run fpga-vblank-latch.yml --repo NigelBreslaw/MiSTer-MagiK --ref main
```

The built RBF is only one part of the fast hidden-buffer path. Seeing
`menu-magik-vblank-latch.rbf` on `/media/fat` does not mean MagiK is using it.
The known-good activation sequence is:

1. Copy the CI artifact to
   `/media/fat/mister-magik/experiments/menu-magik-vblank-latch.rbf`.
2. Load that RBF through Main's command path, not with an external loader:

   ```bash
   scripts/mister run "printf 'load_core /media/fat/mister-magik/experiments/menu-magik-vblank-latch.rbf\n' > /dev/MiSTer_cmd"
   ```

3. Load the stock-kernel plugin probe module so MagiK has hidden RGB565 slots:

   ```bash
   scripts/mister run "insmod /tmp/mister-magik-plugin-probe/mister_magik_plugin_probe.ko"
   ```

4. Start the launcher with the surviving fast backend:

   ```bash
   MISTER_PRESENT_BACKEND=fpga-vblank-latch-hidden scripts/run-rust.sh launcher 0
   ```

5. Verify support with `fpga-latch-report` or `fpga-latch-post-report`. The
   MagiK latch commands `0x57` and `0x58` should report supported status/magic.

If `/tmp/mister-magik/status.json` reports `composition_state=full-slint`, or
if `mister_magik_plugin_probe` is absent from `/proc/modules`, the launcher is
running the normal fallback path even if the experimental RBF file is present.

The launcher path relies on Rust-owned framebuffer setup:

- Set the Linux framebuffer mode.
- Clear/paint the intended frame.
- Route buffer 0 with the FPGA `SET_FBUF` command.
- Keep the route alive while the launcher runs.

Supported launcher rendering paths are:

- Default/fallback: cached RGB565 rendering plus dirty copies into `/dev/fb0`.
- Experimental: `MISTER_PRESENT_BACKEND=fpga-vblank-latch-hidden`, which uses
  the stock-kernel plugin for fast hidden-slot writes and the experimental Menu
  RBF to latch the selected buffer on HDMI vblank.

Main-mediated present request/ack and FIFO present experiments are retired; do
not use them for current device work.

MagiK chooses the UI framebuffer size with `MISTER_UI_FB_SIZE`:

- `auto` is the production default. 1080p-class HDMI output uses a 960x540
  RGB565 framebuffer, while native 720p output keeps a 1280x720 framebuffer.
- `960x540` forces the existing half-1080p framebuffer.
- `1280x720` forces a 1280x720 RGB565 framebuffer while keeping the active HDMI
  output and FPGA scan route unchanged.

Keep `auto` as the default until forced 1280x720 has current device evidence.
For visual evidence, use `scripts/capture-tear-pattern-video.sh` or
`scripts/capture-arcade-scroll-video.sh` with `--ui-fb-size 1280x720`; their
probe files record the requested capture mode and the encoded video geometry,
because USB capture devices can advertise one mode and write another.

Debugging tip: use the desktop Analytics live stream for continuous launcher
inspection. It consumes the producer-side `framebuffer_stream_v1` path from the
running `mister-magik-fb`, so the agent does not repeatedly read `/dev/fb0`.
For one-shot evidence, capture framebuffer PNGs through the MagiK agent with
`scripts/mister agent framebuffer-capture OUT.png --json OUT.json`. Captures are
useful only while the UI is running; after exit, fbcon can redraw the login
console into `/dev/fb0`.

When HDMI looks zoomed, cropped, banded, black, or otherwise different from the
agent framebuffer capture, preserve the state and collect both sides before
restarting anything:

```bash
scripts/mister agent framebuffer-capture OUT.png --json OUT.json
scripts/mister display-read --unsafe-spi
```

`display-read` uses the production `mister-magik-fb read` command to read the
live `UIO_GET_VRES` and `UIO_GET_FB_PAR` FPGA values. It touches the HPS/FPGA
SPI path, so the host wrapper requires `--unsafe-spi` when Main or Slint owns
`/dev/mem`. Use it as a targeted incident probe, not as a polling loop.

On the current MagiK launcher path, `UIO_GET_VRES` may normally report
`529x240` even when HDMI output is visually correct and `UIO_GET_FB_PAR` reports
the expected `960x540` RGB565 framebuffer. This was still true after a clean
manual reboot during the 2026-07-07 HDMI bad-state investigation. Treat
`529x240` as context, not as a bad-state detector.

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

Linux-side HDMI audio is written through `/dev/MrAudio`. The old standalone
`audio-tone` probe was removed from the production command surface; validate
audio through the video path instead. Do not expect ALSA to expose real HDMI
audio on this device.
