# Sprite Effects Catalog

Experimental full-screen RGB565 sprite/object effects live beside the camera
catalog. They are benchmark/demo scenes only; they are not launcher defaults.

## Usage

List labels:

```bash
mister-magik-fb sprite-effects
```

Interactive picker with HUD:

```bash
scripts/run-rust.sh sprite-effects 0
```

Automated benchmark:

```bash
scripts/profile-sprite-effects.sh SPRITE-FX-SMOKE --deploy-fast --mode mega \
  --segment-secs 1 --fb-format 565 --preview-format raw-rgb565 \
  --visual-captures 0 --replace-label
```

Results append to `history/toolchain-bench/results-sprite-effects.tsv`; raw traces
and logs go under `build/sprite-effect-profiles/`.

## Labels

The first catalog contains these 20 labels in stable order:

1. `sprite-zoom-toward-camera`
2. `sprite-shrink-into-distance`
3. `multi-sprite-large-object`
4. `boss-parts-assemble`
5. `sprite-priority-foreground`
6. `sprite-clipping-window`
7. `drop-shadow-copy`
8. `blob-contact-shadow`
9. `invincibility-flicker`
10. `afterimage-trail`
11. `motion-smear-repeats`
12. `exploding-sprite-debris`
13. `tile-chunks-fly-apart`
14. `particle-sparkle-burst`
15. `bullet-hell-ornaments`
16. `rotating-sprite-card`
17. `sprite-sheet-flipbook-logo`
18. `palette-swapped-variants`
19. `mirrored-sprite-reflections`
20. `object-overload-flicker`

## Trace Schema

The raw trace writes one row per frame:

```text
effect frame elapsed_us wall_us cpu_us cpu_pct draw_us present_us vsync_us
clear_us background_us projection_us image_blit_us sprite_us post_us hud_us
sprite_count sprite_pixels particle_count flicker_skip_count
vsync_source vsync_period_us vsync_miss_streak
```

The summary TSV keeps the camera timing metrics and adds average/p95/max sprite
counters for object density and intentional flicker/skip behavior.

## Implementation Notes

- Renderers are host-testable pure Rust in `magik-gui/src/sprite_effects.rs`.
- The demo scene uses the same 960x540 FPGA-scaled framebuffer path as
  `camera-effects` and presents native RGB565 frames.
- Assets are deterministic procedural pixel-art sprites: ship, card, boss
  parts, tiles, chunks, bullets, sparkles, palette variants, and logo frames.
- Raw RGB565 cached screenshots are optional texture fills for card/reflection
  scenes; deterministic synthetic textures are used when no cache exists.
- Joystick left/right cycles effects. `B` or `Start` exits on a fresh press after
  a short startup grace, so stale held buttons on connected pads do not
  immediately close the picker. `secs > 0` exits on timeout. Auto mode is
  controlled with `MISTER_SPRITE_EFFECTS_AUTO=1`.

## Environment

- `MISTER_SPRITE_EFFECTS=mega|label[,label...]`
- `MISTER_SPRITE_EFFECTS_AUTO=1`
- `MISTER_SPRITE_EFFECTS_SEGMENT_SECS=N`
- `MISTER_SPRITE_EFFECTS_HUD=1`
- `MISTER_SPRITE_EFFECTS_TRACE=/tmp/file.tsv`

## Baseline

Initial smoke rows are stored in
`history/toolchain-bench/results-sprite-effects.tsv` after the device benchmark
is run.
