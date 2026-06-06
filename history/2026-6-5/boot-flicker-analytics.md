# Boot Flicker Analytics

Date: 2026-06-07

## Problem

On production boot the Slint launcher flashes several times before stabilizing.
The rightmost column of pixels was also observed moving rapidly, resembling the
stock MiSTer menu/static animation.

## Analytics added

Boot analytics are opt-in. They are enabled only when this flag exists on the
device:

```text
/media/fat/mister-magik/boot-analytics.enabled
```

When enabled, the Main fork writes parent-side events to:

```text
/tmp/mister-magik-boot-analytics.tsv
```

The Slint child also appends boot events to that TSV, writes stdout/stderr to
`/tmp/mister-magik-slint.log`, and writes launcher frame samples to:

```text
/tmp/mister-magik-launcher-frame-profile.tsv
```

Host collection is through:

```bash
scripts/boot-analytics.sh --deploy
```

Bundles land under `build/boot-analytics/<timestamp>/`.

## Captured bundles

- `build/boot-analytics/20260606T204742Z` - initial parent/child timeline.
- `build/boot-analytics/20260606T205528Z` - added launcher frame timings.
- `build/boot-analytics/20260606T212900Z` - added Main video/OSD events and
  right-edge framebuffer signatures.

## Findings

- There is no Slint crash loop. In all runs, the Slint child stayed alive and no
  `launcher_exited` events were recorded.
- Parent handoff is fast and consistent: Main switches to `tty2`, routes buffer
  0 with `video_fb_enable(1, 0)`, and hides the menu before Slint's first frame.
- The route geometry is full frame on both sides:
  `xoff=0 yoff=0 right=1919 bottom=1079 stride=7680`.
  This argues against a simple 1919-pixel exposed-column geometry bug.
- Slint's first frame is delayed until after cached library DB load. In the
  captured runs, first frame was about 1.45-1.48s after `run_ui_start`.
- Slint reasserts `set_mode_1080p` and `fb_enable_direct` every 10 frames for
  frames 0-170. These reasserts remain suspicious for the visible boot flashing.
- Main's OSD layer is still active after launcher handoff. The expanded capture
  recorded repeated post-handoff events like:
  `main-osd OsdUpdate dirty_lines=3 n=19 is_menu=1 osd_size=8 osdset=0x70000`.
  These continued long after `MenuHide()` and after Slint was rendering.
- The right edge also changes in `/dev/fb0`. `launcher-frame-profile.tsv` showed
  the rightmost 1 and 8 columns toggling between black and nonblack on some
  full-screen Slint redraws, then returning to black on later reassert frames.

## Current hypothesis

Two issues are likely interacting:

1. Main's OSD update path continues after the Slint handoff, so the stock UI is
   not fully quiesced even though the menu is hidden.
2. Slint's first-180-frame mode/fb-route reassert loop can visibly perturb the
   HDMI/fb route and also resets the observed right-edge framebuffer state.

## Next changes to try

- Stop/suppress Main OSD updates while the MagiK launcher child is active.
- Remove the first-180-frame Slint `set_mode_1080p` / `fb_enable_direct`
  reassert loop, leaving only the initial open/route path.
- Re-run `scripts/boot-analytics.sh --deploy` and compare:
  - no post-handoff `main-osd OsdUpdate`;
  - no early reassert events;
  - stable right-edge signatures;
  - no visible boot flashing.
