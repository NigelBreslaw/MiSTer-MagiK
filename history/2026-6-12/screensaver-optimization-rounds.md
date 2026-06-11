# Screensaver Optimization Rounds

## Round 0 - Replace placeholder modes with real screensaver primitives

Command:

```bash
scripts/profile-screensaver.sh SCREENSAVER-REALMODES-R00-20260612 --deploy-fast --mode mega --segment-secs 1 --fb-format 565 --preview-format raw-rgb565 --visual-captures 0
```

Compared with the first smoke in
`history/2026-6-12/preview-effects-implementation-bench.md`, this round moved
the screensaver mega sweep from placeholder-style renderers toward the intended
mode descriptions and improved overall pacing:

| run | frames | avg wall us | p95 wall us | p99 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|---:|
| first smoke | 459 | 39118 | 58207 | 101708 | 354 | 344 | 20048 |
| round 0 | 698 | 25666 | 48832 | 50132 | 404 | 338 | 20428 |

Modes now at or near 60fps in the one-second sweep:

| mode | frames | avg wall us | p95 wall us | p99 wall us | >20 ms |
|---|---:|---:|---:|---:|---:|
| super-scaler-flyby | 60 | 16446 | 16577 | 16943 | 0 |
| warp-tunnel | 61 | 16328 | 16561 | 16625 | 0 |
| sprite-multiplex-parade | 61 | 16297 | 16555 | 16700 | 0 |
| color-clash-gallery | 60 | 16505 | 16589 | 16674 | 0 |
| idle-megademo | 60 | 16509 | 16557 | 16640 | 0 |

Next optimization targets:

| mode | avg wall us | p95 wall us | note |
|---|---:|---:|---|
| phosphor-grid | 49497 | 49642 | too many full-screen post-process passes |
| tilemap-museum | 49223 | 50303 | redraws all 96 scaled tiles every frame |
| scanner-contact-sheet | 45667 | 45684 | dense sheet redraw plus expanded thumbnail |
| starfield-cabinets | 46229 | 46246 | contact-sheet redraw dominates sparse starfield |
| attract-wall | 44253 | 44890 | should retain static slots and animate one slot |

## Round 1 - Fixed-point shared screenshot blitter

Command:

```bash
scripts/profile-screensaver.sh SCREENSAVER-BLITTER-R01-20260612 --deploy-fast --mode mega --segment-secs 1 --fb-format 565 --preview-format raw-rgb565 --visual-captures 0
```

This replaces per-pixel division in the common `blit_scaled` path with
fixed-point source stepping and adds a direct row-copy path for exact-size,
untinted RGB565 blits.

| run | frames | avg wall us | p95 wall us | p99 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|---:|
| round 0 | 698 | 25666 | 48832 | 50132 | 404 | 338 | 20428 |
| round 1 | 892 | 20045 | 32865 | 35133 | 394 | 345 | 20364 |

New/remaining 60fps-ish modes:

| mode | avg wall us | p95 wall us | >20 ms |
|---|---:|---:|---:|
| mvs-carousel | 16510 | 16565 | 0 |
| super-scaler-flyby | 16519 | 16604 | 0 |
| screenshot-rain | 16457 | 16557 | 0 |
| raster-gallery | 16409 | 16586 | 0 |
| warp-tunnel | 16513 | 16568 | 0 |
| sprite-multiplex-parade | 16459 | 16552 | 0 |
| cabinet-marquee | 16522 | 16555 | 0 |
| color-clash-gallery | 16536 | 16590 | 0 |

Next targets:

| mode | avg wall us | p95 wall us | note |
|---|---:|---:|---|
| kefrens-screenshot-bars | 35219 | 35299 | vertical-slice renderer still draws many tiny bands |
| mode7-floor | 32981 | 33011 | full lower-half floor resamples every pixel |
| phosphor-grid | 30412 | 30719 | post-process passes still expensive |
| preview-plasma-collage | 30102 | 30140 | per-tile inner copy still samples per pixel |

## Round 2 - Direct Kefrens vertical-slice copier

Command:

```bash
scripts/profile-screensaver.sh SCREENSAVER-KEFRENS-R02-20260612 --deploy-fast --mode kefrens-screenshot-bars --secs 12 --segment-secs 12 --fb-format 565 --preview-format raw-rgb565 --visual-captures 0
```

This removes thousands of tiny scaled-blit calls from
`kefrens-screenshot-bars`. The mode now draws screenshot slices in a direct
row/bar pass using the row-wave table.

| run | frames | avg wall us | p95 wall us | p99 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|---:|
| round 1 mega kefrens segment | 29 | 35219 | 35299 | 36195 | 29 | 29 | 20364 |
| round 2 focused kefrens | 696 | 17103 | 17239 | 20027 | 696 | 8 | 20268 |

The p95 is still just above the 16.7ms target, but the mode is now close enough
for fine-tuning rather than architectural replacement.

## Round 3 - Mode7 floor strip renderer

Command:

```bash
scripts/profile-screensaver.sh SCREENSAVER-MODE7-R03-20260612 --deploy-fast --mode mode7-floor --secs 12 --segment-secs 12 --fb-format 565 --preview-format raw-rgb565 --visual-captures 0
```

This changes `mode7-floor` to the horizontal-strip strategy from the screensaver
notes: draw every other perspective strip with fixed-point source stepping, then
duplicate it into the next row.

| run | frames | avg wall us | p95 wall us | p99 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|---:|
| round 1 mega mode7 segment | 30 | 32981 | 33011 | 34190 | 30 | 30 | 20364 |
| round 3 focused mode7 | 721 | 16504 | 16646 | 16842 | 32 | 0 | 20376 |

`mode7-floor` is now inside the 60fps target for p95 and has no >20ms frames in
the focused run.

## Round 4 - Cheaper phosphor-grid tint path

Command:

```bash
scripts/profile-screensaver.sh SCREENSAVER-PHOSPHOR-R04-20260612 --deploy-fast --mode phosphor-grid --secs 12 --segment-secs 12 --fb-format 565 --preview-format raw-rgb565 --visual-captures 0
```

This removes the full-screen brighten/darken post-process passes from
`phosphor-grid`. The mode now draws the screenshot mosaic already darkened with
the shared blitter tint and reserves the bright flash for sparse scan lines.

| run | frames | avg wall us | p95 wall us | p99 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|---:|
| current deployed mega phosphor segment | 32 | 31294 | 31597 | 32361 | 32 | 32 | 20548 |
| round 4 focused phosphor | 405 | 29522 | 29621 | 32342 | 405 | 405 | 20068 |

This is a modest but measured improvement. `phosphor-grid` still needs a more
structural retained/atlas approach to reach 60fps.
