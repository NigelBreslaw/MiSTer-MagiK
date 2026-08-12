# Rare HDMI Bad State Investigation - 2026-07-07

## Context

The MiSTer occasionally boots into a rare bad visual state, roughly once every
50 or more reboots. Rebooting clears it, so this session deliberately avoided a
MiSTer reboot while collecting evidence.

Observed HDMI symptoms from the user/capture:

- Screen flickers.
- Horizontal/vertical bars roll down the screen.
- Pixels stretch where the bar passes.
- The picture can look zoomed so only about 25% of the intended screen is
  visible.

Important constraint: agent framebuffer captures are not proof of HDMI output.
The MagiK agent captures `/dev/fb0`; the bad state can exist in the HDMI/video
path while `/dev/fb0` contains a clean Slint frame.

## Diagnostics Used

Use the explicit SPI diagnostic read:

```bash
scripts/mister display-read --unsafe-spi
```

This reads:

- `UIO_GET_VRES`
- `UIO_GET_FB_PAR`

It is intentionally gated with `--unsafe-spi` because it touches the FPGA SPI
path while Main/Slint may own display hardware.

HDMI visibility was checked with the Mac USB capture card:

```bash
/opt/homebrew/bin/ffmpeg -hide_banner -y -f avfoundation -framerate 30 -i "0:none" -frames:v 1 /private/tmp/<name>.png
```

## Key Finding

`UIO_GET_VRES -> 529x240` is suspicious, but it is not a reliable detector for
the bad HDMI state.

During the confirmed bad visual state:

```text
UIO_GET_VRES -> width=529 height=240
UIO_GET_FB_PAR -> fb_w=960 fb_h=540 fb_fmt=0x00d4 fb_en=true
```

After a no-reboot Main re-exec/return to launcher, HDMI looked correct again,
but the register read still reported:

```text
UIO_GET_VRES -> width=529 height=240
UIO_GET_FB_PAR -> fb_w=960 fb_h=540 fb_fmt=0x00d4 fb_en=true
```

After a later manual reboot, with the launcher back in its normal visual state,
the same values still appeared:

```text
UIO_GET_VRES -> width=529 height=240
UIO_GET_FB_PAR -> fb_w=960 fb_h=540 fb_fmt=0x00d4 fb_en=true
```

Therefore:

- `UIO_GET_FB_PAR` can look completely normal in both bad and good-enough
  states.
- `UIO_GET_VRES=529x240` appears to be the normal report for this MagiK
  launcher state on this setup, including after a clean manual reboot.
- Do not build automatic recovery logic from either value alone.

## No-Reboot Levers Tested

### Re-exec Main via menu.rbf

After deploying a new `MiSTer_MagiK` without rebooting, activating it with:

```bash
scripts/mister run "printf 'load_core menu.rbf\n' > /dev/MiSTer_cmd"
```

visually cleared the bad HDMI state immediately according to the user and HDMI
capture. This is the most useful evidence from the session: a full Linux reboot
was not required for this occurrence.

This did not clear the `UIO_GET_VRES=529x240` report.

### `mister_magik_hdmi_power_cycle`

The Main-side diagnostic command toggled ADV7513 HDMI power off/on and left the
launcher active. HDMI remained clean, but:

```text
UIO_GET_VRES -> 529x240
UIO_GET_FB_PAR -> 960x540 RGB565 enabled
```

No conclusion yet about whether this helps from the original bad state, because
by the time it was tested the visual output had already been unwedge by Main
re-exec.

### `mister_magik_video_reinit`

The Main-side diagnostic command suspended the Slint child, disabled the HPS
framebuffer, reran Main's `video_reinit()`, and left the launcher suspended.

After this:

```text
launcher_state=LauncherSuspended
fb_mode="8888 1 1920 1080 7680"
UIO_GET_VRES -> 529x240
UIO_GET_FB_PAR -> fb_w=1920 fb_h=1080 fb_fmt=0x00d6 fb_en=true
```

Resuming MagiK brought the Slint launcher back at:

```text
fb_mode="565 1 960 540 1920"
UIO_GET_FB_PAR -> fb_w=960 fb_h=540 fb_fmt=0x00d4 fb_en=true
```

but the user observed the old Main OSD/menu layer. HDMI capture showed MagiK
rendering again, with the old OSD concern still worth treating as a real
failure mode of this lever. Do not promote this to automatic recovery.

## Code Policy From This Session

Keep diagnostics. Do not add automatic startup repair based on the observed
registers.

Specifically rejected:

- Treating `UIO_GET_VRES=529x240` as proof that HDMI is bad.
- Treating normal `UIO_GET_FB_PAR` as proof that HDMI is good.
- Forcing extra black-frame/framebuffer-route retries during MagiK startup
  based on suspicious runtime geometry samples.
- Keeping a combined "recover and resume" command that tries to fix the state
  without a human observing what changed.

The startup path should continue to use the existing display plan/fallback
behavior. The bad HDMI state needs more evidence from the next live occurrence,
not an automatic workaround.

## What To Capture Next Time

When the rare bad state appears again, do not reboot first.

1. Capture HDMI via the USB capture card.
2. Capture the MagiK agent framebuffer and JSON metadata:

   ```bash
   scripts/mister agent framebuffer-capture /private/tmp/bad-fb.png --json /private/tmp/bad-fb.json
   ```

3. Read display registers:

   ```bash
   scripts/mister display-read --unsafe-spi
   ```

4. Record:

   ```bash
   scripts/mister status
   ```

5. Try attended levers one at a time, with HDMI capture and `display-read` after
   each:

   ```bash
   scripts/mister run "printf 'mister_magik_hdmi_power_cycle\n' > /dev/MiSTer_cmd"
   scripts/mister run "printf 'mister_magik_video_adjust\n' > /dev/MiSTer_cmd"
   scripts/mister run "printf 'mister_magik_video_reinit\n' > /dev/MiSTer_cmd"
   ```

6. If still wedged, re-exec Main without a Linux reboot:

   ```bash
   scripts/mister run "printf 'load_core menu.rbf\n' > /dev/MiSTer_cmd"
   ```

The goal next time is to find the smallest no-reboot operation that changes the
HDMI picture, not to infer state from one register value.

## 2026-08-12 Return-From-Game Occurrence

A separate rare black-screen occurrence was captured after returning from an
FPGA Arcade game to MiSTer MagiK. The launcher framebuffer remained healthy
while USB Video was black, and `UIO_GET_VRES=529x240` again did not distinguish
the failure from normal operation. Reapplying Main's current runtime HDMI
output configuration restored the picture without a reboot.

The retained FPGA record was misleading because its field named for HDMI PLL
lock was connected to the adjustment-PLL LED status. Schema 4 corrects only
that passive observer connection by exporting the real HDMI PLL lock from the
existing wrapper. It does not change the latch, reset, clocks, scaler, SDRAM,
or output path. Main now reasserts the already-selected runtime HDMI output
configuration on the marked game-to-launcher return path.
