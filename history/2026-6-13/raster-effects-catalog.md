# Raster / Palette Effects Catalog

Current policy: effect catalogs are experiments, not release benchmark evidence.
Use `docs/experiments/effects.md` and `scripts/experiments/` for current
commands.

Added 2026-06-13 as the fourth experimental full-screen classic-effects catalog,
beside `camera-effects`, `sprite-effects`, and `text-effects`.

## Usage

List effect labels:

```bash
mister-magik-fb raster-effects
```

Run the joystick-browsable demo on the MiSTer:

```bash
scripts/run-rust.sh raster-effects 0
```

Run the smoke benchmark:

```bash
scripts/profile-raster-effects.sh RASTER-FX-SMOKE --deploy-fast --mode mega \
  --segment-secs 1 --fb-format 565 --preview-format raw-rgb565 \
  --visual-captures 0 --replace-label
```

Results append to `history/toolchain-bench/results-raster-effects.tsv`; raw traces
and logs go under `build/raster-effect-profiles/`.

## Effect Labels

1. `palette-cycling-lava-water-neon`
2. `palette-gradient-sky`
3. `per-scanline-color-bars`
4. `rainbow-raster-bands`
5. `copper-bar-horizontal-glow`
6. `screen-flash-action`
7. `fade-through-indexed-palettes`
8. `day-night-palette-swap`
9. `limited-color-posterize-transition`
10. `dither-dissolve`
11. `ordered-checker-dissolve`
12. `crt-phosphor-fade-trail`
13. `scanline-brightness-pulse`
14. `palette-split-warning-tint`
15. `water-reflection-flipped-wavy-rows`

## Runtime Env

- `MISTER_RASTER_EFFECTS=mega|label[,label...]`
- `MISTER_RASTER_EFFECTS_AUTO=1`
- `MISTER_RASTER_EFFECTS_SEGMENT_SECS=N`
- `MISTER_RASTER_EFFECTS_HUD=1`
- `MISTER_RASTER_EFFECTS_TRACE=/tmp/file.tsv`

## Benchmark Counters

The trace keeps the shared timing buckets:
`clear`, `background`, `projection`, `image_blit`, `sprite`, `post`, and `hud`.

Raster-specific counters:

- `palette_step_count`
- `lut_lookup_count`
- `row_op_count`
- `dither_pixel_count`
- `flash_pixel_count`
- `trail_pixel_count`
- `indexed_pixel_count`
- `reflection_row_count`

## Implementation Notes

- Renderers are host-testable pure Rust in `magik-gui/src/raster_effects.rs`.
- The UI loop uses the native 960x540 RGB565 framebuffer path and presents via
  the same full-screen loop as the other catalogs.
- Indexed and palette-style effects use deterministic synthetic indexed art.
- Preview-cache raw RGB565 images are optional source imagery for posterize,
  dissolve, scanline pulse, warning tint, and water reflection effects.
- CRT phosphor trail keeps a retained RGB565 trail buffer in renderer state.
- Water reflection copies from a scratch frame, flips the upper scene, and applies
  cheap row offsets instead of arbitrary resampling.

## Baseline

Baseline rows are written to `history/toolchain-bench/results-raster-effects.tsv`
after the device smoke benchmark is run.
