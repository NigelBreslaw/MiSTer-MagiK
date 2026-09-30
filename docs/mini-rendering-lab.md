# Mini rendering lab

Implement the shared renderer experiments in `nigel/mini-card-fidelity` without
changing production quality defaults. The implementation plan is:

1. Reuse the real six root-card textures, the cabinet and the production rasterizers.
2. Add deterministic launcher and Arcade storyboards to the existing Mini concept
   session, including reverse travel, interrupted spins and settling.
3. Compare `default` (current), `dithered` (final quantisation), and `rgb888`
   (retain source precision). Keep preparation outside the timed frame loop.
4. Pause and step; switch quality at the same instant; retain fixed-frame captures.
5. Run uninstrumented device cadence separately from captures and profiling.
6. Qualify the selected change in the full application before changing defaults.

## Workloads and variants

`launcher-cards` calls the production `PreparedLauncher` renderer with the actual
six 180x252 RGB565 assets. It uses the production frame preparer with two persistent
tile buffers, split at x=629, and a CPU0 helper while the UI/presenter runs on
CPU1 through the repository thread policy. It spins all six cards right and left,
holds each front card for 180 ms, and unwinds a partial spin. The loop lasts 8040 ms. Its fixed
metadata and public Spleen fixture font do not load the catalog or private fonts.

`default` preserves the production renderer. `dithered` adds destination-space
4x4 ordered quantisation after filtering, alpha composition and angle lighting.
`rgb888` additionally prepares linear-light 2x2 reductions of the existing
360x504 RGB888 sources, retaining RGB8 artwork through the mipmaps and lighting.
Typography, trim and silhouette coverage keep their prepared face values.
All buffers and physical output remain RGB565.

`arcade-transition` calls the production Arcade card reveal timeline and
composition. The destination is the committed 960x540 HDMI visual-baseline PNG,
not a live catalog. Filtered variants prepare the resting cabinet region with
the same quantisation as the moving image, preserving text and game pixels;
this prevents an old RGB565 snapshot from changing cabinet quality at settlement.
It opens for 1000 ms, holds for 200 ms, closes for 1000 ms,
and holds Home for 200 ms. `default` preserves nearest sampling. `dithered` uses
2D minification levels and bilinear/level interpolation prepared from RGB565;
`rgb888` prepares those levels from the original RGB888 cabinet. Both filtered
variants apply ordered dithering after cabinet composition; chrome/list fades
retain production behavior. The cabinet mipmap averages are currently in sRGB;
linear-light mip generation and higher-precision fades remain separate experiments.

The cabinet RGB888 asset is from the accepted Blender PNG, flattened and clamped
with the existing asset recipe. Repacking this source reproduces the existing
RGB565 SHA256 `6f1cde9ae3ea57f2f2e1c33746da5323e047a174928d675bc64a17bd214f1e45`.
No new Blender render, camera or material change is required. RGB888 is source
precision, not the framebuffer format. These labs require the resolved 960x540
HDMI render geometry; they reject other geometry without changing device modes.

## Fast visual iteration

Run from this worktree:

```sh
scripts/magik concept launcher-cards
scripts/magik concept arcade-transition --preset rgb888
```

The interactive session supports `preset default`, `preset dithered`,
`preset rgb888`, `pause`, `resume`, `step`, `restart`, `capture`, and `quit`.
Changing quality in the same lab preserves its time and paused state. To select
another workload, use `select launcher-cards` or `select arcade-transition`.
The host uses supported native delivery, display ownership and authoritative
captures. It does not revive removed framebuffer-scene-lab host commands.

For a host-only frame (portable reference, not physical timing):

```sh
scripts/cargo run --manifest-path magik/probe/Cargo.toml --bin mini-magik --   --render-lab launcher-cards --preset rgb888 --time-ms 210 --output /tmp/cards.ppm
```

The same exporter accepts `arcade-transition`. Review card times 0, 210, 419,
420, 600, 3810, 7410 and 7830; Arcade times 0, 250, 500, 750, 1000 and 1700.
PPM files are decoded RGB565 output. Keep captures and generated evidence ignored.

## Device qualification

Run one explicitly selected case at a time:

```sh
scripts/magik check concept --app mini-magik --concept launcher-cards   --preset dithered --production-build
scripts/magik check concept --app mini-magik --concept arcade-transition   --preset rgb888 --production-build
```

`--production-build` selects opt-level 3, thin LTO, 32 codegen units and Cortex-A9
flags, matching the full app's optimization policy. The normal Mini profile stays
fast (opt-level 2, no LTO). Rendering labs cannot qualify on that fast profile.
Two uninstrumented 30-second runs require valid physical evidence, zero repeated
vblanks/drops/rejections, CPU below 150% and RSS at most 128 MiB. Captures and
control review happen after measurement. `--profile` is a separate attribution
run and cannot qualify cadence. Record artifact hash, geometry, build profile,
render/transfer/frame-to-present times and retained storage. Passing Mini proves
this isolated workload, not whole-app scheduling or live input parity. Mini waits
for both parallel tiles; production also has a render-ahead coordinator, so their
scheduling and latency remain distinct. The initial single-thread reference
measured about 30 FPS and 20.6 ms render time; it is not the qualification path.

Compare the moving card's banding, texture stability, silhouettes, label quality
and the final moving-to-resting frame. Prefer a consistent visual pipeline; do
not accept a quality switch on settlement without measured evidence that a
consistent path cannot meet the deadline.

## Cached Arcade experiment

Arcade also accepts `--preset cached`. This uses the RGB888 filtered renderer
once during selection to prepare 61 poses at 60 Hz. Only the changing y=77..540
band is stored (about 52 MiB); carousel scratch and the mip pyramid are released.
The UI freezes during this preparation, and `context.preparation_ms` records its
cost. Playback copies the corresponding pose; reverse travel reuses the same
poses. Arbitrary paused times round to the nearest 60 Hz pose. This is an explicit
memory/latency tradeoff for the fixed fixture, not a production cache policy.

```sh
scripts/magik check concept --app mini-magik --concept arcade-transition   --preset cached --production-build
```

A production adoption must address preparation latency, invalidation when live
screen data changes, input cancellation and retained memory before enabling it.
The lab does not hide preparation time or describe cached playback as live
filtering performance. `cached` is rejected for the carousel and other concepts.

## Initial device evidence, 2026-09-30

All rows below used the production-matched ARM profile on a confirmed 960x540
HDMI render surface. Each pair is two separate uninstrumented 30-second windows.
The figures describe this fixed Mini fixture, not the full application's cadence.

| Workload | Mode | Physical FPS, two windows | Repeated vblanks | Qualification |
| --- | --- | --- | --- | --- |
| Carousel, parallel tiles | current | 59.167 / 59.367 | 25 / 19 | failed |
| Carousel, final SIMD packing | RGB888 | 30.633 / 30.567 | 881 / 883 | failed |
| Arcade reveal | current | 30.167 / 30.150 | 895 / 896 | failed |
| Arcade reveal, live filtering | RGB888 | 11.381 / 11.381 | 1461 / 1461 | failed |
| Arcade reveal, prepared poses | cached RGB888 | 60.000 / 59.998 | 0 / 0 | passed |

The cached reveal rendered in 1.84 / 1.80 ms on average, with p99 2.13 / 2.10 ms.
Both windows recorded 1800 unique latch presentations, zero latch drops/rejections,
about 100.2% process CPU, peak RSS 83,132,416 bytes (79.3 MiB), and 57,336,960
bytes of retained scene storage. Preparation took 7061 / 7043 ms; it remains
visible in result metadata. This proves high-precision filtered/dithered playback
can meet physical 60 Hz for the fixed fixture after preparation. It does not
qualify its seven-second synchronous preparation for production interaction.

Ignored evidence directories under `build/magik-results/`:

- `20260930T092743Z-a4391e96267d`: parallel current carousel.
- `20260930T094300Z-235230e7a258`: RGB888 carousel with final SIMD packing.
- `20260930T094507Z-b749ef903fed`: current Arcade reveal.
- `20260930T094718Z-eefc4e8374f1`: live-filtered RGB888 Arcade reveal.
- `20260930T095740Z-e02d5715b9ab`: qualified cached RGB888 Arcade reveal.

Each directory retains artifact identity, raw metrics, result summaries and
FPGA-latched PNG captures. Earlier direct-filtering results precede the final
resting-cabinet consistency repair; the cached qualification includes that repair.
The carousel's remaining work is final-conversion cost and presentation scheduling.
Its failed Mini candidates are not evidence that banding is unavoidable at 60 Hz.
The cached reveal supplies the concrete memory/preparation tradeoff to assess
before attempting a production change.

## Live scanline follow-up, 2026-09-30

`scanline` is an Arcade-only live renderer using the same RGB888 source,
premultiplied mipmaps, bilinear/trilinear filtering, timeline and destination-space
RGB565 quantisation as `rgb888`. It does not cache animation frames. Rust prepares
coordinates and source bounds once per frame and reuses two horizontal rows per
mip level. The ARM kernels batch adjacent texel pairs, vertical interpolation,
colour conversion, and RGB565 background/list composition. Two persistent workers
split the changing content at y=311 through Mini's existing CPU1/CPU0 policy.
Each owns its output and roughly 11.4 KiB of row scratch; the helper owns cloned
immutable source buffers. Shutdown closes the request channel and joins the worker.

The original exact ordered quantiser simplifies without changing a pixel. For
scaled channel value `s` and Bayer threshold `0<t<256`, the reference test
`remainder*256 > t*255` is equivalent to `remainder >= t`. Consequently the
quantised value is `floor((s+255-t)/255)`. With `n=s+256-t`, the bounded calculation
`(n+(n>>8))>>8` is exact and fits sixteen-bit intermediates. The NEON path now
uses that identity rather than the original quotient/remainder/comparison sequence.
The exploratory 8 KiB lookup table was removed. This same quantiser serves the
opt-in launcher quality paths; production quality defaults remain unchanged.
A small shared sRGB transfer table also replaces per-sample gamma powers during
RGB888 card reduction. It matches the old encoding for every tested input pair
and all reductions of the six real card images, but does not remove the remaining
cost of preparing the complete launcher fixture.

The live follow-up improved Arcade from 11.381 FPS to 50.267 / 50.233 FPS.
It still failed strict 60 Hz: 292 / 293 repeated vblanks in two 30-second windows.
Render averages were 10.60 / 10.62 ms, with p99 17.38 / 17.43 ms, before transfer
and presentation. CPU was 142.7 / 142.8%. The context's `arcade_tile_max_us` array
contains per-scene maxima for primary rendering, secondary rendering and waiting;
these include the lead-in and are attribution, not window percentiles.

## Accelerated runtime cache

`cached-fast` uses the live two-worker renderer to generate the existing 61-pose
cache on the device. It uses no build-time animation assets and adds no image
files to the binary. Its retained changing band and reverse playback are identical
to `cached`. The worker, duplicate sources and mip scratch are released after
preparation. This remains a cache of the fixed complete Mini screen composition,
including static fixture labels, counts, clock and game pixels; it is not a
qualified production policy for live data.

Mini now retains one resting launcher snapshot per quality (at most 3 MiB), so
switching Arcade experiments does not regenerate all six launcher faces. The
existing `context.preparation_ms` remains the complete scene-construction timer;
`preparation_stages_ms` makes this reuse explicit. It measures preparation rather
than input-to-first-latched-frame latency. Fully cold fixture creation is still
reported, and no loading or first-presentation guarantee is inferred from it.

| Final workload | Physical FPS, two windows | Repeated vblanks | Cadence qualification |
| --- | --- | --- | --- |
| Arcade, live scanline | 50.267 / 50.233 | 292 / 293 | failed |
| Arcade, accelerated runtime cache | 60.000 / 60.000 | 0 / 0 | passed |
| Launcher cards, shared exact quantiser and RGB888 | 30.949 / 31.667 | 872 / 850 | failed |

For `cached-fast`, total cold scene preparation was 2999 ms: 2011 ms creating
the launcher fixture, 32 ms decoding the destination, 23 ms preparing the cabinet,
13 ms preparing the worker, and 912 ms generating frames. With the launcher snapshot
already available, total preparation was **984 ms**: 3 ms snapshot copy, 32 ms
destination decode, 22 ms cabinet preparation, 13 ms worker setup and 908 ms frame
generation, plus small remaining overhead. This is just under the user's one-second
preparation budget for this asset and has little margin. Production would need to
supply the already prepared launcher rather than regenerate this fixture, and
measure complete entry latency with the actual graphics and live screen data.

Both cached-fast windows presented 1800 unique frames with zero latch drops or
rejections. Playback averaged 1.83 / 1.79 ms, p99 2.12 / 2.08 ms; CPU was about
100.2%, peak RSS 83,804,160 bytes (79.9 MiB), scene storage 57,336,960 bytes.
The live path retained 10,425,694 scene bytes; its process also owns the bounded
fixture snapshot cache. These scene counters do not replace measured process RSS.

The shared quantiser reduced RGB888 carousel rendering to 17.20 / 16.90 ms on
average, p99 19.37 / 19.09 ms. It still misses the deadline after presentation
costs. This is not evidence that consistent dithering is impossible at 60 Hz;
its projection/filtering and scheduling remain separate work.

Portable checks cover band canaries and invalid bounds, mip boundaries, edges,
reverse travel, worker completion and cache/reference equality. FPGA-latched
960x540 midpoint captures for live and cached variants match the scalar reference
pixel-for-pixel. Focused library/Mini clippy, renderer tests, Mini controls and
host preset checks passed. ARM builds use the production-matched profile. An
exploratory all-target clippy run also exposed pre-existing test-module placement
lints in `arcade_card.rs` and `launcher_texture.rs`; the focused library checks
pass, and those unrelated layout changes were left alone.

Final evidence under ignored `build/magik-results/`:

- `20260930T111717Z-73de9e0f40d5`: final live scanline measurement and captures.
- `20260930T112453Z-57254650c912`: cached-fast measurement, preparation attribution
  and captures; artifact SHA256
  `9ea5b14ae0feb86e97e04ae38c0d7d84a707677bdd1334a05cbb45a4883214c6`.
- `20260930T112728Z-fe51318544ec`: final RGB888 launcher-card measurement.
- `20260930T111354Z-b78ab4c52fc1`: separate instrumented tile-cost profile.

To try either Arcade variant after the normal launcher is restored:

```sh
scripts/magik concept arcade-transition --preset scanline --production-build
# At concept>, switch to the accelerated cache:
preset cached-fast
```

The first cold Mini selection still creates the fixture. Switching to cached-fast
then reuses its launcher snapshot. `quit` closes the test session and restores
persistent Mini under the native session contract; run `scripts/magik stop` to
return to the Dev launcher. No production default, firmware, display mode or
full-application deployment was changed by this follow-up.
