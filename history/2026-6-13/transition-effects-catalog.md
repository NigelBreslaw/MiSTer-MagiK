# Transition Effects Catalog

Current policy: effect catalogs are experiments, not release benchmark evidence.
Use `docs/experiments/effects.md` and `scripts/experiments/` for current
commands.

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
13. `crt-power-off-fast-snap`
14. `crt-power-off-hot-line`
15. `crt-power-off-center-dot`
16. `crt-power-off-phosphor-bloom`
17. `crt-power-off-wobble-collapse`
18. `burn-in-ghost-crossfade`
19. `glitchy-sprite-priority-reveal`
20. `cabinet-marquee-light-sweep`

The extra CRT power-off variants are a small gallery for finding which pieces of
the classic tube shutdown feel work best on the RGB565 framebuffer: fast
deflection snap, lingering hot line, center-dot bloom, phosphor afterglow, and
wobbly deflection collapse.

## Runtime Env

- `MISTER_TRANSITION_EFFECTS=mega|label[,label...]`
- `MISTER_TRANSITION_EFFECTS_AUTO=1`
- `MISTER_TRANSITION_EFFECTS_SEGMENT_SECS=N`
- `MISTER_TRANSITION_EFFECTS_HUD=1`
- `MISTER_TRANSITION_EFFECTS_TRACE=/tmp/file.tsv`

Focused CRT-off gallery:

```bash
MISTER_TRANSITION_EFFECTS=crt-power-off-vertical-collapse,crt-power-off-fast-snap,crt-power-off-hot-line,crt-power-off-center-dot,crt-power-off-phosphor-bloom,crt-power-off-wobble-collapse \
MISTER_TRANSITION_EFFECTS_HUD=1 mister-magik-fb ui transition-effects 0
```

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
- The transition HUD uses a fixed 5x7 bitmap label font so long effect names stay
  readable while cycling variants.
- Burn-in ghost keeps a retained RGB565 buffer in renderer state.

## Baseline

Baseline rows are written to `history/toolchain-bench/results-transition-effects.tsv`
after the device smoke benchmark is run.
