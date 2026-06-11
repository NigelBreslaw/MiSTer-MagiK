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
