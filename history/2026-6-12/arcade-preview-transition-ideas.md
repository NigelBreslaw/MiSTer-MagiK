# Arcade preview transition ideas - 2026-06-12

Goal: find new effects for moving between arcade screenshots that feel native to
classic arcade and early home-computer visuals, while fitting the current raw
preview blitter and MiSTer framebuffer path.

## Current implementation

The transition control surface is `magik-gui/src/screenshot_transitions.rs`.
The current `mega` set is:

- `cut`
- `fade`
- `wipe`
- `slide`
- `zoom`
- `scanline`
- `checker`
- `dissolve`
- `crt-beam-wipe`
- `mosaic-resolve`

The expensive work happens in
`magik-gui/src/ui_runner/ui_frame_target.rs`, mostly through
`blit_raw_preview_transition`. The best path is RGB565 source to RGB565 cached
target. It can avoid RGB expansion, and it only repaints/presents the preview
aperture. Effects should therefore prefer:

- row, column, or tile order changes
- integer math and lookup tables
- whole-slice copies when possible
- cached-RAM reads and sequential framebuffer writes
- RGB565 operations that can later grow NEON helpers

Avoid live `/dev/fb0` read-modify-write. The 2026-06-08 and 2026-06-11 arcade
band-copy trials showed that reading the write-combined framebuffer roughly
doubles present cost.

Recent `REVIEW-PR3-AFTER-MEGA-20260611` transition frames on RGB565 averaged:

| Effect | Custom draw avg |
|---|---:|
| `zoom` | 5.9 ms |
| `wipe` | 5.9 ms |
| `crt-beam-wipe` | 6.1 ms |
| `scanline` | 6.9 ms |
| `checker` | 7.4 ms |
| `dissolve` | 7.6 ms |
| `fade` | 7.7 ms |
| `slide` | 8.2 ms |
| `mosaic-resolve` | 11.4 ms |

Wall time stayed near vsync, but the custom draw budget is real. New effects
should try to land closer to `wipe` than `mosaic-resolve`.

## Inspiration

Classic home machines were good at effects that are synchronized to scanlines,
palette changes, blits, and integer address tricks. Useful references:

- Raster/copper bars: animated color bars and per-scanline palette changes on
  Atari, C64, Amiga, Atari ST, and similar machines.
  https://en.wikipedia.org/wiki/Raster_bar
- Old-school demo effects: scrollers, starfields, smooth per-scanline waves,
  plasma, shadebobs, vector graphics, and chunky-pixel effects.
  https://en.wikipedia.org/wiki/Demo_effect
- Amiga blitter/Copper: rectangular block copies, masks, barrel shifts, lines,
  and video-synchronized register changes.
  https://en.wikipedia.org/wiki/Amiga_Original_Chip_Set
- Film/TV wipes that also suit UI transitions: barn-door, iris, matrix, star,
  and clock wipes.
  https://en.wikipedia.org/wiki/Wipe_(transition)
- Vector arcade displays: line-drawn images, phosphor fade, bright redraw, and
  games such as Asteroids, Tempest, and Star Wars.
  https://en.wikipedia.org/wiki/Vector_monitor
- C64 demo tricks: sprite scrollers, sprite multiplexing, FLD row spacing,
  FLI color density, Tec-Tec per-line x offsets, VSP, and linecrunching.
  https://en.wikipedia.org/wiki/Commodore_64_demos
- VIC-II hardware: 16 colors, 8 sprites per scanline, raster interrupts, smooth
  scrolling, sprite expansion, and per-register color/sprite controls.
  https://en.wikipedia.org/wiki/MOS_Technology_VIC-II
- Atari 2600 "racing the beam": no framebuffer, tiny RAM, per-scanline drawing,
  visible timing scars, and sprite reuse after the beam has passed.
  https://www.wired.com/2009/03/racing-the-beam/
- ZX Spectrum/MSX attribute graphics: 8x8 or 8x1 color attribute limits, FLASH,
  BRIGHT, and special-effect timing changes between display lines.
  https://en.wikipedia.org/wiki/Attribute_clash
- Raster parallax and line scroll: strips of an image can scroll at different
  offsets/rates; row/column scroll appeared on Sega arcade boards since Space
  Harrier/System 16 and on Capcom CPS/Irem/Taito boards.
  https://en.wikipedia.org/wiki/Parallax_scrolling
- Galaga '88: blue warp capsules move the player between dimensions, giving a
  very on-theme arcade reason for a brief starfield/warp transition.
  https://en.wikipedia.org/wiki/Galaga_%2788
- Plasma and chunky-pixel demo effects: sine/noise fields, color cycling,
  rotozoomers, tunnels, wobblers, bump maps, and feedback filters.
  https://en.wikipedia.org/wiki/Plasma_effect

## Best candidates

### 1. `copper-bars`

Horizontal neon bands sweep through the preview while revealing the new image.
Each row computes a small sine/triangle band intensity from `local_y + t`.
Rows inside the bright band show `current`, rows just behind it show a cheap
blend, and untouched rows remain `previous`.

Why it fits: row-local, branch-light, visually classic, and easy to make MiSTer
MagiK-branded with teal/magenta highlights. For RGB565, precompute one 320-entry
row alpha table per frame and apply it across columns. A later NEON version can
blend 8 or 16 RGB565 pixels per chunk.

### 2. `venetian-blinds`

Reveal alternating 4 px or 8 px horizontal slats first, then fill the gaps. A
vertical variant can reveal columns. This is the home-computer version of a TV
wipe, and it looks good on low-resolution screenshots.

Why it fits: mostly rectangular row/column copies. For horizontal blinds, rows
that are fully `current` can be copied from source; rows that are still hidden
can copy previous or black. Only the moving edge needs blending/brightening.

### 3. `barn-door`

Open from the center outward, or close old image into the center and reveal the
new one behind it. This is a two-sided wipe, not a per-pixel shader.

Why it fits: two rectangles per row, cheap thresholds, very readable during fast
list movement. A vertical barn-door variant is almost free relative to `wipe`.

### 4. `iris`

Circular reveal from the center, optionally with a bright CRT rim. Use squared
distance against an expanding radius. To reduce per-pixel cost, precompute a
320x320 `u16` distance map once, or compute per 4x4 tile.

Why it fits: iconic, different from current rectangular `zoom`, and pairs well
with arcade cabinet framing. It is more math-heavy unless it uses a lookup map,
so this is a good second-tier spike.

### 5. `clock-wipe`

Reveal a radial sector like a sweeping analog clock hand. Precompute angle or
quadrant thresholds in a 320x320 map, then each frame compares `angle_index <=
progress_index`.

Why it fits: formal wipe language, old-TV feel, cheap after precompute. It also
gives a nice "selecting the next machine" impression without looking like a web
UI animation.

### 6. `sprite-strips`

Break the screenshot into 8 px horizontal strips. Each strip slides from a
slightly different direction or delay, like a bunch of blitter objects snapping
into place.

Why it fits: Amiga/ST blitter-object energy, but implemented as row slices. Avoid
per-pixel blend; copy strip spans with integer offsets. This should be much
cheaper than generic `slide` if special-cased for native RGB565 screenshots.

### 7. `starfield-warp`

Use a sparse table of pseudo-random stars/streaks over black or previous image
for the first half, then reveal current with a wipe/dissolve in the second half.
Inspired by Galaga/Galaxian space backdrops and demoscene starfields.

Why it fits: the star overlay is tiny compared with the preview area. The reveal
can reuse `wipe` or `dissolve`, but the effect feels much more arcade.

### 8. `vector-redraw`

Flash to a dark phosphor background, draw bright line segments or an outline grid
over the old image, then reveal the current screenshot. Think Asteroids/Tempest
redraw rather than bitmap fade.

Why it fits: line drawing is sparse and can use integer Bresenham. It should not
try to edge-detect the screenshot in real time. Use a fixed line motif: cabinet
outline, diagonal vectors, or a rotating wire diamond.

### 9. `palette-cycle`

Instead of blending images, quantize/solarize the current image through a small
set of RGB565 masks for the first frames: dark blue, magenta/green, full color.
This mimics palette cycling and limited CLUT machines without actually having a
palette framebuffer.

Why it fits: bit masks and channel shifts are friendly to RGB565 and NEON. It is
especially good as an accent applied to another reveal, such as `copper-bars`.

### 10. `raster-tear`

Each scanline samples the current image with a small x offset from a sine table,
with offsets shrinking to zero as progress reaches 1. This is the "wobbler"
family in miniature.

Why it fits: one x offset per row, integer table lookup, source sampling stays
row-local. It is more expensive if every pixel samples both previous and current,
so use it as a current-only reveal after a quick cut/black flash.

### 11. `tile-loader`

Reveal 16x16 tiles in ROM/address order, not random order: left-to-right by
character row, serpentine, or interleaved banks. Add a one-frame bright flash as
each tile appears.

Why it fits: looks like old hardware loading tile RAM, but implementation is the
same family as `checker` with a deterministic order table. Can copy whole tiles
instead of hashing per pixel.

### 12. `venetian-copper`

Combine `venetian-blinds` with a moving copper-bar highlight on the active slat
edges. This is likely the strongest "classic but polished" composite.

Why it fits: mostly row copies plus tiny highlighted edge rows. It should have a
better cost/look ratio than full-screen per-pixel dissolve.

### 13. `attribute-flash`

Quantize the current image into 8x8 cells for the first frames. Each cell chooses
two dominant-ish RGB565 colors or a fixed pair from a small bright palette, then
the cell flips between old/current or dark/bright on a checker timing. This is a
ZX Spectrum/MSX tribute, but used deliberately as a transition rather than as a
defect.

Why it fits: one decision per tile instead of one decision per pixel. The cell
palette can be approximated cheaply from the center pixel or four samples. The
effect should be strongest at 80-140 ms, not a long animation.

### 14. `tec-tec`

Shift each scanline of the current screenshot by a sine-table x offset, with the
offset collapsing toward zero. This is the C64 Tec-Tec / per-line x-position
family, and it is less chaotic than `raster-tear` if the sine phase is coherent.

Why it fits: one offset lookup per row, then mostly contiguous row spans. For the
center area, copy `current[x + offset]`; for exposed edges, use previous or a
dark phosphor color. Avoid blending except on the final 1-2 px seam.

### 15. `linecrunch`

Reveal the current screenshot by changing vertical line spacing: start with every
8th row repeated into chunky bands, then progressively uncrunch to true rows.
This echoes C64 linecrunching/FLD and the "screen is being rebuilt by the beam"
feel.

Why it fits: row selection only. The renderer can fill output row `y` from source
row `quantize(y, step)` where `step` shrinks from 8 to 1. With RGB565 native
screenshots, whole rows or spans can be copied.

### 16. `racing-beam`

A bright horizontal beam sweeps downward. Above the beam is current, below is
previous/dark, and the left edge gets a few black timing bars that disappear as
the frame stabilizes. This references Atari 2600/VCS scanline programming and
its visible timing budget scars.

Why it fits: a cheaper cousin of `crt-beam-wipe`, but with a more specific
identity. It is mostly row thresholding plus a few left-edge rect fills.

### 17. `sprite-multiplex`

Reveal the current screenshot as several fake "sprite slots" reused down the
screen: 24x21 or 16x16 blocks appear in repeated vertical lanes, then fill into
the final image. Add one-pixel bright borders on newly appeared blocks.

Why it fits: deterministic block ordering, no hash per pixel, and directly
inspired by C64/VIC-II and Amiga sprite reuse. Copy whole blocks from cached
source; use a precomputed block order table.

### 18. `row-scroll-parallax`

Split the screenshot into 8 or 16 horizontal strips. Far strips move only a few
pixels while near strips slide farther, all settling into the final screenshot.
This is the transition version of raster parallax/line scroll and early arcade
pseudo-depth.

Why it fits: strip copy with per-strip offsets. It should be cheaper and more
legible than arbitrary per-pixel warping, and it connects visually to Moon
Patrol/Space Harrier/System 16/CPS-era raster tricks.

### 19. `super-scaler-pop`

Start current as a few coarse sprite-like chunks scaled from the center, then
snap them to native size. Think Sega Super Scaler or Space Harrier objects flying
toward the camera, but constrained to the preview aperture.

Why it fits: nearest-neighbor integer scaling already matches the project's
visual language. Keep it to 4 or 9 chunks and use 2x/1x copies where possible;
avoid arbitrary fractional scaling.

### 20. `mask-blit`

Use a precomputed 1-bit mask pattern that changes each frame: diagonal slash,
diamond, maze, logo-ish "M", or tiny cabinet grille. The mask selects previous
or current at tile or word granularity. This is closer to Amiga masked blits than
to alpha blending.

Why it fits: boolean selection is the cheap path. A NEON version can load source
vectors from previous/current and use a vector compare/select mask, but the
scalar version is still simple.

### 21. `phosphor-decay`

Instead of crossfading old to new, decay the previous screenshot to green/blue
phosphor trails for 2-3 frames, draw a sparse vector grid, then cut/reveal the
current screenshot. This is a stronger version of `vector-redraw`.

Why it fits: limited duration and sparse overlays. The decay can use RGB565 bit
masks and shifts rather than full alpha math.

### 22. `plasma-mask`

Use a tiny 64x64 or 80x80 animated plasma field as a reveal mask, not as a full
color shader. Pixels/tiles where `plasma(x, y, t) < threshold(progress)` show
current; the rest show previous or a darkened previous. Optionally tint newly
revealed edges with magenta/cyan.

Why it fits: if the plasma field is low-res and tiled/scaled, most work is table
lookup and thresholding. It gives a classic demo look without doing expensive
multi-sine RGB math across the whole 320x320 preview each frame.

### 23. `moire-rings`

Reveal using expanding rings from two nearby centers. Where the two ring fields
intersect, draw a brighter edge or flip from previous to current. This borrows
from old moire-circle demo effects.

Why it fits: precompute two distance maps or use coarse 4x4 tiles. The visual is
distinct from `iris` because it has interference bands rather than one clean
circle.

### 24. `kefrens-curtain`

Use vertical bars whose x positions are displaced by a sine curve per y row,
like a compact Kefrens/raster curtain. The bars sweep across the preview and
leave current image behind them.

Why it fits: it is mostly a threshold against `local_x + wave[local_y]`, so it
is cheaper than arbitrary texture warping. It can share lookup tables with
`copper-bars` and `tec-tec`.

## Full-screen screensaver ideas

These are idle-mode effects for the full 960x540 render target, reusing the
optimized screenshot cache rather than decoding PNGs during animation.

Relevant cache facts from `history/2026-6-11/rgb565-raw-preview-bench.md`:

- Native raw RGB565 previews live under
  `screenshot-magik/raw565-nearest-320x320/*.rgb565`.
- The raw565 file format is `MM56501\0`, width, height, 16-byte-aligned stride,
  then little-endian RGB565 rows.
- A MiSTer cache build produced 904 files, 0 failures, about 147 MB total.
- Runtime raw565 loading removed resize cost and cut preview load total to about
  2.5 ms on average.
- Once in memory, raw565 screenshots can be copied into the RGB565 cached frame
  without RGB8 expansion.

Screensaver rules of thumb:

- Keep a rolling RAM set of maybe 16-64 screenshots, not the whole cache.
- Prefer one write-only pass into the cached frame, then present sequential rows
  or broad rects to `/dev/fb0`.
- Animate by moving/scaling/cropping cached screenshots, not by reading the live
  framebuffer.
- Use table-driven color accents and masks. Full 960x540 per-pixel blending is
  possible but should be reserved for sparse or low-frequency effects.
- Treat 320x224-ish arcade screenshots as a feature: 3 across fits 960 wide,
  and 2 rows of 224 plus gutters fits 540.

### 1. `attract-wall`

A living arcade wall: 3 columns by 2 rows of screenshots, each in its own
cabinet-like slot. Every few seconds one slot performs a transition from the
list above and swaps to a new game.

Why it fits: six 320-wide images map cleanly to 960 pixels. Most frames are
static, so only animated slots need redraw. It makes direct use of the cached
screenshots as screenshots, which is good for visual browsing.

### 2. `mvs-carousel`

Four or six screenshots sit in a slow horizontal carousel like a Neo Geo MVS
multi-slot attract display. The centered game is full brightness; side games are
darkened or palette-cycled. Title/system text can stay optional and tiny.

Why it fits: mostly row-span copies with x offsets. The side-card darkening can
be RGB565 channel shifts. No need for fractional transforms if positions snap
through integer coordinates.

### 3. `super-scaler-flyby`

Screenshots fly toward the viewer as chunky sprite cards, inspired by Sega Super
Scaler/Space Harrier. Cards start small near the horizon, grow through 1x/2x,
then peel offscreen.

Why it fits: use a small set of integer scales and nearest-neighbor copies.
Avoid arbitrary scale every frame. A 160x112 half-size prepass could be cached
in RAM per active screenshot.

### 4. `starfield-cabinets`

A full-screen starfield runs behind a few drifting screenshot panels. Panels
tilt only in the cheap sense: horizontal strips are offset by a small table to
fake perspective.

Why it fits: stars are sparse; panels are cached row copies. The strip offsets
reuse `row-scroll-parallax`/`tec-tec` machinery without full texture mapping.

### 5. `screenshot-rain`

Small screenshot thumbnails fall down the screen in staggered columns, like a
coin-op Matrix made of tiny attract screens. Old thumbnails fade by darkening
each frame or by switching to lower-brightness copies.

Why it fits: thumbnails can be pre-resized to 80x56 or 160x112 in RAM. Motion is
rect copies into a cached frame; background can be a cheap vertical gradient or
solid dark color.

### 6. `tilemap-museum`

The screen is a 12x8 or 15x9 grid of tiny screenshot tiles. Groups of tiles flip,
swap, or palette-flash in deterministic patterns. Occasionally the grid zooms
into one selected screenshot.

Why it fits: deliberately tile-based, cache-friendly, and easy to update in
chunks. It turns the preview cache into a video memory tile set.

### 7. `raster-gallery`

One large screenshot fills the center while copper/raster bars sweep behind it.
Every bar pass reveals strips of the next screenshot until the whole screen has
changed.

Why it fits: full-screen look with only row-wise operations. The screenshot can
be letterboxed or repeated/mirrored into a 960x540 backdrop.

### 8. `kefrens-screenshot-bars`

Vertical bars cut from different screenshots follow sine-wave x positions. Each
bar is a 16 or 32 px slice, so the screen becomes a moving curtain of arcade
imagery.

Why it fits: copy vertical slices from cached images, modulated by a row-wave
table. It has strong demo flavor and never needs per-pixel alpha.

### 9. `preview-plasma-collage`

Use a low-res plasma field to choose which screenshot each 16x16 tile samples
from. As the plasma moves, the full screen becomes a liquid collage of 4-8 games.

Why it fits: the plasma is a tile selector, not an RGB shader. Each tile copies
from one cached image. This can look wild while staying mostly block-copy based.

### 10. `phosphor-grid`

Show a dark vector grid over a slowly changing mosaic of screenshots. Every few
seconds, the grid bright-flashes and the underlying screenshots decay to a green
or cyan phosphor tone before new images appear.

Why it fits: sparse line drawing plus RGB565 mask/shift decay. The screenshots
are static for most frames, so this is cheap and very arcade-room.

### 11. `warp-tunnel`

Use screenshots as texture bands inside a fake tunnel: concentric rectangles or
rings pull toward the center. A new screenshot is assigned to each depth band.

Why it fits: implement as rectangles, not polar sampling. Draw nested rect edges
or thick bands from cropped screenshot rows. It reads as a tunnel without the
cost of a real rotozoomer.

### 12. `mode7-floor`

A pseudo-Mode-7 floor made of screenshot tiles scrolls toward the viewer. The
top half can be starfield/logo; the bottom half is a perspective-ish tile grid.

Why it fits: do it as horizontal strips. Each strip has a precomputed source
step and x-repeat. A low-res intermediate buffer, e.g. 320x180, can be scaled to
960x540 or copied into the lower portion.

### 13. `scanner-contact-sheet`

A bright scan beam moves over a dense contact sheet of screenshots. As it passes
over each thumbnail, that thumbnail briefly expands, brightens, or shows a
transition frame.

Why it fits: the beam is a row band and the contact sheet is mostly static.
Only the active thumbnail needs extra work. It is the most "library browser as
screensaver" option.

### 14. `sprite-multiplex-parade`

Treat screenshots as giant hardware sprites. Rows of small preview sprites move
left/right at different speeds, with only 8-12 sprites "active" per band. The
same screenshot slot can reappear further down the screen with a different tint.

Why it fits: directly maps to sprite multiplexing: a small number of cached
images reused at many y positions. Integer positions and no alpha required.

### 15. `cabinet-marquee`

Top and bottom bands are moving screenshot strips, while the center shows one
featured screenshot with a slow `copper-bars`/`attribute-flash` treatment. Think
arcade marquee plus attract monitor.

Why it fits: banded layout reduces the amount of full-screen motion. It can
share code with arcade-list row copying and transition accents.

### 16. `random-access-loader`

Screenshots appear in ROM-address order using chunky 16x16 loads, with a tiny
fake address counter or checksum motif. The whole screen fills, clears, and
fills again from a different shuffled page.

Why it fits: this is `tile-loader` scaled up to the full screen. Copy whole
tiles, and make the old-computer loading behavior the art direction.

### 17. `color-clash-gallery`

Divide the screen into 8x8 or 16x16 attribute cells. Each cell shows a tiny
piece of a screenshot but is constrained to a two-color bright palette for a few
seconds, then the true-color screenshot bursts through.

Why it fits: the "limitation" is the effect. Cell decisions are cheap and the
true-color reveal can happen by tile copy.

### 18. `idle-megademo`

Cycle several of the above as one screensaver: 20-30 seconds each, with a shared
pool of cached screenshots and a single low-res effect table store.

Why it fits: this mirrors actual demo disks/megademos and prevents any one
effect from overstaying. It also provides a practical harness for testing the
same primitives used by screenshot transitions.

### Suggested screensaver spike order

1. `attract-wall` - highest value, simplest, showcases the library.
2. `scanner-contact-sheet` - mostly static with one animated active region.
3. `screenshot-rain` - straightforward thumbnail sprite system.
4. `kefrens-screenshot-bars` - distinctive and copy-oriented.
5. `starfield-cabinets` - arcade feel, sparse background cost.
6. `preview-plasma-collage` - most fun table-driven collage.
7. `mode7-floor` - needs careful strip math but could be a showpiece.
8. `idle-megademo` - compose after 2-3 primitives exist.

## Implementation notes

- Add enum entries in `PreviewTransitionEffect`, update `MEGA`, labels, parsing,
  and script validation in `scripts/profile-preview-scroll.sh`.
- Keep the generic RGB fallback, but optimize RGB565 native-size first.
- Split the RGB565 transition fast path by effect. Many candidates can avoid
  `sample_raw565` per pixel by copying row slices or tiles directly.
- Add small precompute helpers:
  - `row_wave_table(progress, h)` for copper/raster effects
  - `tile_order_16x16()` for tile-loader/matrix effects
  - `circle_distance_320()` and `angle_sector_320()` for iris/clock
- Prefer candidate-specific RGB565 functions over a giant per-pixel `match`.
  `venetian-blinds`, `linecrunch`, `tec-tec`, `sprite-strips`,
  `row-scroll-parallax`, and `mask-blit` can all be written as row/span/block
  copies.
- Treat `attribute-flash`, `palette-cycle`, and `phosphor-decay` as optional
  post-process accents. They can share RGB565 channel-mask helpers.
- Treat `plasma-mask`, `moire-rings`, and `kefrens-curtain` as table-driven
  mask effects. Do not implement them as full dynamic RGB shaders first.
- Keep long effects out of the default list. Many of these feel best at
  120-220 ms; beyond that the list selection can feel laggy.
- Benchmark with:

```bash
scripts/profile-preview-transition-mega.sh LABEL --deploy-fast --segment-secs 3 --transition-ms 220
```

Then use fixed visual review:

```bash
scripts/profile-preview-scroll.sh 20 held-scroll LABEL --skip-build --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition EFFECT --visual-captures 4
```

## Suggested spike order

1. `venetian-blinds` - cheapest and likely immediately usable.
2. `copper-bars` - best identity fit; good NEON blend candidate.
3. `sprite-strips` - blitter-era feel, copy-oriented.
4. `tile-loader` - low risk alternative to per-pixel dissolve.
5. `tec-tec` - C64 line wobble, row-local and cheap if span-copied.
6. `attribute-flash` - distinctive 8-bit color-block identity.
7. `row-scroll-parallax` - arcade raster depth, likely readable.
8. `iris` - beautiful if lookup-table cost behaves.
9. `clock-wipe` - same lookup machinery as iris.
10. `plasma-mask` - classic demo flavor if kept low-res/table-driven.
11. `vector-redraw` / `phosphor-decay` - visually distinctive, sparse overlay.
12. `raster-tear` - spicy, but easiest to make visually noisy.
