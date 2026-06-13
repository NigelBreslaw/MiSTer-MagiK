# Transition Effects Catalog

Added 2026-06-13 as the fifth experimental full-screen classic-effects catalog,
beside `camera-effects`, `raster-effects`, `sprite-effects`, and `text-effects`.
It is separate from production arcade preview transitions.

## Usage

List effect labels:

```bash
mister-magik-fb transition-effects
```

Run the joystick-browsable demo on the MiSTer:

```bash
scripts/run-rust.sh transition-effects 0
```

Run the smoke benchmark:

```bash
scripts/profile-transition-effects.sh TRANSITION-FX-SMOKE --deploy-fast \
  --mode mega --segment-secs 1 --fb-format 565 --preview-format raw-rgb565 \
  --visual-captures 0 --replace-label
```

Results append to `history/toolchain-bench/results-transition-effects.tsv`; raw
traces and logs go under `build/transition-effect-profiles/`.

## Effect Labels

1. `venetian-blinds-wipe`
2. `horizontal-shutter-close`
3. `vertical-arcade-door-close`
4. `iris-circle-open-close`
5. `radial-spoke-wipe`
6. `mosaic-block-in-out`
7. `tilemap-page-flip`
8. `checkerboard-reveal`
9. `starfield-warp-transition`
10. `tunnel-zoom-transition`
11. `screen-shake-flash`
12. `crt-power-off-vertical-collapse`
13. `burn-in-ghost-crossfade`
14. `glitchy-sprite-priority-reveal`
15. `cabinet-marquee-light-sweep`

## Runtime Env

- `MISTER_TRANSITION_EFFECTS=mega|label[,label...]`
- `MISTER_TRANSITION_EFFECTS_AUTO=1`
- `MISTER_TRANSITION_EFFECTS_SEGMENT_SECS=N`
- `MISTER_TRANSITION_EFFECTS_HUD=1`
- `MISTER_TRANSITION_EFFECTS_TRACE=/tmp/file.tsv`

## Benchmark Counters

The trace keeps the shared timing buckets:
`clear`, `background`, `projection`, `image_blit`, `sprite`, `post`, and `hud`.

Transition-specific counters:

- `mask_cell_count`
- `revealed_pixel_count`
- `hidden_pixel_count`
- `source_a_pixel_count`
- `source_b_pixel_count`
- `shake_offset_px`
- `flash_pixel_count`
- `warp_sample_count`
- `ghost_pixel_count`
- `glitch_band_count`

## Implementation Notes

- Renderers are host-testable pure Rust in `magik-gui/src/transition_effects.rs`.
- The UI loop uses the native 960x540 RGB565 framebuffer path and presents via
  the same full-screen loop as the other catalogs.
- Two source frames are rendered from cached raw RGB565 previews when available,
  with deterministic synthetic fallback art.
- Mask-heavy effects use rows, blocks, and spans instead of per-pixel blends.
- Burn-in ghost keeps a retained RGB565 buffer in renderer state.

## Baseline

Baseline rows are written to `history/toolchain-bench/results-transition-effects.tsv`
after the device smoke benchmark is run.
