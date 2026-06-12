# Camera effects catalog - 2026-06-13

This checkpoint adds a first-pass full-screen RGB565 catalog for classic arcade
background/camera tricks. It is intentionally experimental: the effects are not
launcher defaults yet, and several are deliberately unoptimized so the benchmark
can point at the right bottlenecks before we polish them.

## Commands

```bash
# List effect labels
mister-magik-fb camera-effects

# Interactive device picker, left/right cycles effects
scripts/run-rust.sh camera-effects 0

# Automated MiSTer benchmark with CPU + renderer timing buckets
scripts/profile-camera-effects.sh CAMERA-FX-SMOKE --deploy-fast --mode mega \
  --segment-secs 1 --fb-format 565 --preview-format raw-rgb565 \
  --visual-captures 0 --replace-label
```

The scene uses the same 960x540 FPGA-scaled UI framebuffer path as the launcher
and screensaver loops. It renders native RGB565 frames, reads cached raw-rgb565
arcade previews when available, and falls back to deterministic synthetic images
if no preview cache is present.

## Baseline

Smoke run: `CAMERA-FX-SMOKE`, 960x540 RGB565, 1 second per effect.
Results are in `history/toolchain-bench/results-camera-effects.tsv`.

Near or at 60fps on the first pass:

| effect | fps | p95 wall | avg CPU |
|---|---:|---:|---:|
| sprite-starfield | 62.1 | 16.6 ms | 21% |
| perspective-road | 61.2 | 16.5 ms | 38% |
| infinite-cloud-bank | 62.0 | 16.5 ms | 23% |
| city-lights-parallax | 60.9 | 16.5 ms | 24% |
| isometric-tile-drift | 62.0 | 16.5 ms | 33% |

Main optimization targets for a later pass:

| effect | fps | p95 wall | dominant bucket |
|---|---:|---:|---|
| column-scroll-shimmer | 15.0 | 67.5 ms | image/projection resample |
| rotate-zoom-background | 18.9 | 53.2 ms | projection |
| pseudo3d-horizon-bend | 22.7 | 44.3 ms | background bend |
| row-scroll-water | 30.4 | 32.9 ms | projection |
| mode7-rotating-floor | 35.2 | 28.5 ms | projection |

## Notes for later

- Keep the CPU/timing buckets when optimizing; they make "60fps but too much CPU"
  visible.
- The slow effects are mostly full-frame resampling. Likely fixes are retained
  layers, row-pair/strip renderers, lower-resolution maps, and replacing
  per-pixel projection with lookup tables.
- `camera-effects` is separate from `arcade-effects`: the latter is still the
  screenshot transition picker on the real arcade list surface.
