# Preview Transitions And Screensavers Implementation Smoke

Date: 2026-06-12

This records the first device smoke after adding the expanded screenshot
transition list and the full-screen screensaver scene.

## Commands

```bash
scripts/profile-preview-transition-mega.sh TRANSITIONS-SMOKE-20260612 --deploy-fast --segment-secs 1 --transition-ms 220 --fb-format 565 --preview-format raw-rgb565 --visual-captures 0
scripts/profile-screensaver.sh SCREENSAVER-SMOKE-20260612 --skip-build --mode mega --segment-secs 1 --fb-format 565 --preview-format raw-rgb565 --visual-captures 0
```

## Transition Smoke

Overall:

| case | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| arcade | 1303 | 25828 | 39015 | 677 | 660 | 32332 |

The original 10 effects stayed close to the 60fps target. The new first-pass
effects are mostly around a 30fps cadence because they currently use generic
per-pixel masks.

Worst transition targets:

| effect | frames | avg wall us | p95 wall us | >20 ms |
|---|---:|---:|---:|---:|
| clock-wipe | 22 | 44681 | 60373 | 15 |
| starfield-warp | 23 | 37142 | 41619 | 21 |
| super-scaler-pop | 31 | 37797 | 41515 | 28 |
| venetian-copper | 31 | 36001 | 39749 | 28 |
| vector-redraw | 32 | 30065 | 39068 | 21 |

## Screensaver Smoke

Overall:

| case | frames | avg wall us | p95 wall us | p99 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|---:|
| screensaver | 459 | 39118 | 58207 | 101708 | 354 | 344 | 20048 |

Only `sprite-multiplex-parade` and `super-scaler-flyby` were close to 60fps in
the first pass. The rest need atlas/precompute or lower-cost composition before
they can become defaults.

Worst screensaver targets:

| mode | frames | avg wall us | p95 wall us | p99 wall us |
|---|---:|---:|---:|---:|
| kefrens-screenshot-bars | 10 | 101962 | 101908 | 101908 |
| preview-plasma-collage | 13 | 78949 | 78881 | 78881 |
| color-clash-gallery | 18 | 55606 | 55556 | 55556 |
| random-access-loader | 20 | 50016 | 51513 | 51513 |
| warp-tunnel | 20 | 48146 | 52153 | 52153 |

## Optimization Rounds From The Smoke

1. Replace `clock-wipe` atan2 gating with an integer octant/edge sweep lookup.
2. Specialize row/span transition effects instead of calling the generic mask per pixel.
3. Convert tile/mask transitions to block-copy renderers with precomputed tile order.
4. Precompute radial/ring/plasma maps for `iris`, `moire-rings`, `plasma-mask`, and `super-scaler-pop`.
5. Add a thumbnail atlas for screensaver contact-sheet modes.
6. Precompute screensaver plasma/color-clash backgrounds at lower cadence and scroll/copy them.
7. Rewrite `kefrens-screenshot-bars` as cached horizontal strip copies instead of scaled blits per row.
8. Add frame-to-frame retained layers for gallery screensavers; redraw only moving overlays.
9. Tune `mega` ordering and default eligibility so effects over target are opt-in until optimized.
10. Run the full 60fps benchmark matrix after each optimization PR and promote effects as they pass.
