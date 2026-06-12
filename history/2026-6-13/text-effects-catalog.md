# Text Effects Catalog

Experimental full-screen RGB565 text effects live beside the camera and sprite
catalogs. They are benchmark/demo scenes only; they are not launcher defaults.

## Usage

List labels:

```bash
mister-magik-fb text-effects
```

Interactive picker with HUD:

```bash
scripts/run-rust.sh text-effects 0
```

Automated benchmark:

```bash
scripts/profile-text-effects.sh TEXT-FX-SMOKE --deploy-fast --mode mega \
  --segment-secs 1 --fb-format 565 --preview-format raw-rgb565 \
  --visual-captures 0 --replace-label
```

Results append to `history/toolchain-bench/results-text-effects.tsv`; raw traces
and logs go under `build/text-effect-profiles/`.

## Labels

The catalog contains these 50 labels in stable order:

1. `insert-coin-blink-cadence`
2. `high-score-initials-cursor-pulse`
3. `sine-wave-text-scroller`
4. `per-letter-bounce`
5. `per-letter-palette-chase`
6. `text-zoom-from-horizon`
7. `letter-tiles-snap-into-grid`
8. `logo-shimmer-palette-cycle`
9. `score-counter-rolling-digits`
10. `ready-go-slap-burst`
11. `typewriter-dialogue-reveal`
12. `vector-stroke-draw-on-text`
13. `continue-countdown-panic`
14. `trackball-signature-initials`
15. `grawlix-speech-bubble`
16. `rasterbar-title-backing`
17. `palette-cycled-text-fill`
18. `plasma-filled-logo-text`
19. `victory-quote-textbox`
20. `continue-screen-tip-ticker`
21. `finish-him-impact-prompt`
22. `neo-geo-boot-slogan-flash`
23. `extend-letter-bubbles`
24. `phrase-spelling-bonus-meter`
25. `powerup-letter-icon-pop`
26. `intermission-caption-card`
27. `attract-instruction-pages`
28. `wave-announcement-banner`
29. `get-ready-voice-text-sync`
30. `dot-matrix-credit-roll`
31. `amiga-copperbar-scrolltext`
32. `amiga-rainbow-raster-title`
33. `amiga-copper-split-credits`
34. `amiga-blitter-bob-letter-swarm`
35. `amiga-bob-path-scrolltext`
36. `amiga-shadebob-writing-text`
37. `amiga-infinite-bob-glyph-trail`
38. `amiga-kefrens-bar-text-wipe`
39. `amiga-moire-circle-title-mask`
40. `amiga-plasma-scrolltext-fill`
41. `amiga-keftales-zoom-texture`
42. `amiga-rotozoom-logo-text`
43. `amiga-wobbler-flag-text`
44. `amiga-texture-tunnel-text-ribbon`
45. `amiga-vector-line-font-spin`
46. `amiga-filled-vector-logo-turntable`
47. `amiga-glenz-transparent-text`
48. `amiga-blenk-metal-text-sweep`
49. `amiga-rubber-gel-text-twist`
50. `amiga-scrolltext-explode-reassemble`

## Trace Schema

The raw trace writes one row per frame:

```text
effect frame elapsed_us wall_us cpu_us cpu_pct draw_us present_us vsync_us
clear_us background_us projection_us image_blit_us sprite_us post_us hud_us
glyph_count glyph_pixels tile_count vector_segment_count bob_count
palette_step_count hidden_glyph_count scroll_offset
vsync_source vsync_period_us vsync_miss_streak
```

The summary TSV keeps the camera timing metrics and adds average/p95/max text
counters for glyph density, tiles, vector segments, Amiga-style bobs, palette
steps, hidden glyphs, and scroll position.

## Implementation Notes

- Renderers are host-testable pure Rust in `magik-gui/src/text_effects.rs`.
- The demo scene uses the same 960x540 FPGA-scaled framebuffer path as
  `camera-effects` and `sprite-effects`, then presents native RGB565 frames.
- Assets are deterministic procedural glyphs and generated raster/plasma/moire
  fields. No ripped fonts, logos, or demo assets are used.
- Optional raw RGB565 cached screenshots are sampled only as texture color in
  tunnel-style scenes; deterministic synthetic textures are used when no cache
  exists.
- Joystick left/right cycles effects. `B` or `Start` exits on a fresh press after
  a short startup grace, so stale held buttons on connected pads do not
  immediately close the picker. `secs > 0` exits on timeout. Auto mode is
  controlled with `MISTER_TEXT_EFFECTS_AUTO=1`.

## Environment

- `MISTER_TEXT_EFFECTS=mega|label[,label...]`
- `MISTER_TEXT_EFFECTS_AUTO=1`
- `MISTER_TEXT_EFFECTS_SEGMENT_SECS=N`
- `MISTER_TEXT_EFFECTS_HUD=1`
- `MISTER_TEXT_EFFECTS_TRACE=/tmp/file.tsv`

## Source Anchors

The Amiga addendum is grounded in public demoscene writeups and terminology:

- [Amiga demos](https://en.wikipedia.org/wiki/Amiga_demos)
- [Demo effect](https://en.wikipedia.org/wiki/Demo_effect)
- [Demoscene terminology](https://it.wikipedia.org/wiki/Demoscene#La_terminologia)
- [Raster bar](https://en.wikipedia.org/wiki/Raster_bar)
- [Amiga Original Chip Set / Copper](https://en.wikipedia.org/wiki/Amiga_Original_Chip_Set)
- [Blitter object](https://en.wikipedia.org/wiki/Blitter_object)

## Baseline

Initial smoke rows are stored in
`history/toolchain-bench/results-text-effects.tsv` after the device benchmark is
run.
