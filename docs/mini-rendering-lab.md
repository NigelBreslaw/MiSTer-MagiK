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

## Live optimisation round, 2026-09-30

This round retains the accepted RGB888-source image quality, final ordered RGB565
quantisation, original interpolation rounding, and two-worker thread policy.
It adds no artwork, animation poses, atlases or render-ahead queue. Production
quality defaults are unchanged. All physical measurements below use the Cortex-A9
release-device build at 960x540, with two separate 30-second windows per step.
No sub-agents were used during implementation.

Mini now uses the existing production vblank pacer before checking the posted
slot, avoiding continuous polling through available slack. Motion advances with
monotonic elapsed time, rather than one synthetic 16.667 ms tick per completed
frame. Pause excludes paused time; step/bookmarks remain deterministic. The host
qualification gate checks that animation time follows the device measurement
clock, including warmup. This prevents slow playback from appearing qualified.
`--installed-sha256` now verifies and reuses the requested ready Mini artifact
instead of rebuilding it.

Retained improvements and their independently measured impact:

| Change | Workload | FPS, two windows | Render p99, ms | Process CPU |
| --- | --- | --- | --- | --- |
| Pacing and monotonic motion baseline | Cards | 32.433 / 35.080 | 19.09 / 18.74 | 111 / 115% |
| Four vertical NEON quantiser lanes; hoist Bayer phase | Cards | 45.800 / 43.033 | 16.83 / 16.92 | 135 / 130% |
| Render primary tile into scene buffer; merge only helper band | Cards | 59.798 / 59.833 | 14.25 / 13.90 | 148 / 147% |
| Pacing and monotonic motion baseline | Arcade | 53.367 / 53.067 | 17.38 / 17.41 | 111 / 111% |
| Dispatch base fade endpoints outside pixel loops | Arcade | 54.667 / 55.333 | 16.39 / 15.99 | 109 / 109% |
| Exact signed 16-bit horizontal variable-weight interpolation | Arcade | 56.831 / 56.231 | 15.40 / 15.51 | 111 / 111% |
| Direct 1:1 rows; skip pixels covered by opaque game overlay | Arcade | 56.731 / 56.867 | 15.38 / 15.25 | 104 / 105% |
| SIMD premultiplied over, row phase hoist, overlay endpoints | Arcade | 59.898 / 59.967 | 13.66 / 13.40 | 100 / 99% |
| Guarded known-black background composition | Arcade | 59.965 / 59.967 | 11.99 / 11.91 | 92 / 93% |

The card copy reduction saves approximately 1.29 MB of frame-buffer traffic per
frame. The fixed Home background is seeded once and restored on scene reset;
the changing primary region is rendered directly into the scene buffer. Only
the helper's x=629..934, y=120..495 region is merged. Both workers remain joined
before presentation. Arcade's black-background shortcut is enabled only when
rounded Home fade weight is zero and the entire cabinet render bounds are inside
the subject region that the base pass wrote black. Its zero-alpha behavior and
RGB565 background decoding remain exactly equal to the scalar reference.

Several plausible arithmetic/layout changes were discarded after full-workload
measurement. Smaller instruction counts alone were not a useful selection rule:

| Rejected trial | Physical result | Reason |
| --- | --- | --- |
| Fused filtering/lighting | Cards 45.300 / 43.867 FPS | No useful gain over four quantiser lanes |
| Four-column microtiles | Cards 45.867 / 45.400 FPS | No useful gain for added complexity |
| Constant-weight signed delta blend | Cards 59.731 / 59.731 FPS; CPU 153 / 152% | Higher CPU and p99 |
| Signed delta perspective filter | Cards 59.767 / 59.800 FPS | No useful improvement |
| Direct 16-column strips | Cards 59.800 / 59.800 FPS | No useful improvement |
| 32-column blocked output | Cards 59.700 / 59.767 FPS; CPU 155 / 154% | Higher CPU and p99 |
| 16-column blocked output | Cards 59.731 / 59.600 FPS; CPU 157 / 156% | Higher CPU, p99 and misses |
| Direct vblank wait policy | Arcade 59.933 / 59.898 FPS; 2 / 3 repeats | Worse cadence than retained pacing |
| Defer hidden card body colours | Cards 59.765 / 59.800 FPS; CPU 153 / 152% | No gain despite reference equality |

### Final retained implementation

The same final artifact was used for both workloads:
`67d01282b252f8807286b5d4fb3089b7e0f5d54ae4b4bfbe8411d649139131ab`.
These runs include the bounded miss records described below.

| Workload | FPS | Repeated refreshes | Mean / p99 render, ms | Process CPU | Peak RSS |
| --- | --- | --- | --- | --- | --- |
| Cards, window 1 | 59.800 | 6 | 12.59 / 14.46 | 148.6% | 42.88 MiB |
| Cards, window 2 | 59.767 | 7 | 12.49 / 14.48 | 148.1% | 42.88 MiB |
| Arcade, window 1 | 59.967 | 1 | 7.44 / 11.89 | 92.6% | 42.88 MiB |
| Arcade, window 2 | 59.967 | 1 | 7.47 / 11.92 | 92.9% | 42.88 MiB |

Both workloads still fail the strict zero-repeat cadence gate. There were zero
latch drops/rejections; the reported repeats are physically observed refreshes
without a new presentation. Scene storage remains 19,548,522 bytes for cards and
10,425,694 bytes for live Arcade; no complete-frame cache is introduced.
The final initial/midpoint/boundary FPGA-latched captures match the pre-change
reference captures pixel-for-pixel. This does not replace full-app live-input,
variable data or CRT qualification.

Evidence remains ignored under `build/magik-results/`:

- `20260930T141221Z-05b1371be29b`: final cards, two windows and control/capture checks.
- `20260930T141542Z-092265babf15`: final Arcade, same installed artifact.
- `20260930T124822Z-e694675e94fb`: card copy-reduction comparison.
- `20260930T133554Z-67ced19077e3`: Arcade known-black comparison.
- `20260930T141851Z-3be07e681cde`: restore normal Dev launcher.

The complete per-step run identifiers, hashes and rejected-trial snapshots are
under ignored `outputs/mini-card-fidelity/optimisation/`.

### What the current miss record can establish

Mini retains at most 32 records per measurement window in `context.late_frames`.
Each records animation time, render/transfer/frame-to-present wall time,
primary/helper/wait wall times, and the number of repeated refreshes observed.
`tile_max_us` still includes scene lead-in; `tile_us` in a miss record instead
refers to that frame. This is bounded stage attribution, not a scheduler trace.

All six misses in card window 1 were separated by about 5.03 seconds; window 2
showed the same periodicity, plus one adjacent miss. They occurred at different
animation poses. For example, the window-1 miss at animation time 14,215 ms had
19,959 us total render time, 11,579 us primary time, 16,114 us helper time, 7,245 us
helper wait and 1,197 us transfer. This suggests a periodic disturbance but does
not identify the competing task or prove preemption. Arcade's single miss in each
window supplies insufficient samples to establish the same periodicity.

CPU percentages and p99 discard the ordering and correlation needed to explain
rare misses. Worker wall time includes both execution and off-CPU delay. The
current trace does not record worker wakeup-to-first-run latency, per-thread CPU
time, competing scheduler/IRQ events, or a precise target/actual latch timeline.
Those are the remaining diagnostic gaps. No affinity or priority changes were
made on the strength of the observed periodicity.

A follow-up diagnostic should correlate one frame ID across enqueue, worker
start/end, join, transfer/post, target refresh and actual latch. Record per-thread
CPU time alongside wall time. A bounded scheduler-event trace should distinguish
runnable waiting, blocking, preemption and interrupt time, retaining history
before a miss and exporting it after measurement. Kernel tracing availability
must be checked through a typed device operation. Measure instrumentation overhead
with tracing disabled/enabled before treating that evidence as representative.
Optimisation is paused after this round for discussion of that diagnostic.

### Focused validation

Portable scene tests compare live scanline output with the scalar reference,
including every millisecond across fade/identity/coverage boundaries. Native ARM
NEON harnesses compare projection, variable-weight interpolation, final alpha
composition, base/overlay endpoints and known-black rows with the original/scalar
arithmetic. They exercise tails, canaries and destination dither phases. These
host ARM64 checks establish arithmetic parity; Cortex-A9 timings and captures
come from the actual device rather than host extrapolation.

```sh
scripts/cargo test --manifest-path crates/framebuffer-scenes/Cargo.toml --lib
scripts/cargo test --manifest-path crates/visual-concepts/Cargo.toml --lib
scripts/cargo test --manifest-path magik/probe/Cargo.toml --bin mini-magik concepts::tests
clang -O3 -Wall -Wextra -Werror crates/framebuffer-scenes/tests/launcher_neon_parity.c -o /tmp/launcher-neon-parity
clang -O3 -Wall -Wextra -Werror crates/framebuffer-scenes/tests/arcade_neon_parity.c -o /tmp/arcade-neon-parity
clang -O3 -Wall -Wextra -Werror crates/framebuffer-scenes/tests/cabinet_neon_parity.c -o /tmp/cabinet-neon-parity
```

## Cold launcher preparation profile, 2026-09-30

The preparation-only diagnostic starts the existing 99 Hz CPU sampler before
`Scene::new_with_worker_setup`, stops it as soon as scene construction completes,
and renders one paused initial frame. It does not start a motion measurement,
run a storyboard or compare the old implementation. Stage clocks are gated by
the existing `launcher-profile` feature and an explicit one-shot action. Hardware
counters are disabled in this mode. Serialization and flamegraph generation happen
after the preparation timer and sampling window.

```sh
scripts/magik check concept --app mini-magik --concept launcher-cards --preset rgb888 --production-build --profile-preparation
```

Measured on the Cortex-A9, production-matched 960x540 build:

- Total cold scene preparation: **2078 ms**, instrumented.
- Six-card launcher fixture: **2066 ms**; worker setup: **7 ms**.
- Process CPU consumed during scene preparation: **2,082,219 us**, approximately
  one fully occupied core; the helper starts only near the end.
- CPU sample count: **203**. Leaf samples: artwork surface 132 (65.0%), RGB888
  reduction iterator 32 (15.8%), texture coverage construction 13 (6.4%), texture
  mip construction 7 (3.4%), RGB8 texture retention 6 (3.0%). Optimisation inlines
  most inner surface arithmetic, so the sampled symbol is the enclosing surface
  function rather than each individual helper. All collected stacks contained
  just one symbol, so this build supplies leaf attribution rather than complete
  call chains; the stage timers supply the nested cost breakdown. These sample
  percentages are attribution estimates, not exact elapsed-time percentages.

The mutually exclusive high-level fixture stages account for its elapsed time:

| Work | Wall time |
| --- | ---: |
| Construct generic compact/detail faces that are then replaced | 796.5 ms |
| Construct the retained RGB888 compact/detail faces | 1226.3 ms |
| Chrome, scratch/buffers, first complete resting render, other fixture setup | about 43 ms |

The constructor makes the initial generic faces before its RGB888 branch replaces
both faces. All six cards use that branch in this fixture. No generic back is
retained because RGB888 artwork is present. Therefore the initial 796.5 ms is
avoidable work on this particular path; it is approximately 38% of total scene
preparation. No optimisation has been applied as part of this profiling task.

Within the retained RGB888 face construction, measured nested stages are:

| Work | Calls | Wall time |
| --- | ---: | ---: |
| Linear-light 360x504 to 180x252 artwork reduction | 12 | 338.8 ms |
| Surface, labels, silhouette coverage and initial textures | 12 | 789.8 ms |
| Restore RGB8 source precision and rebuild mip levels | 12 | 90.0 ms |
| Remaining conversion/copy/replacement work | | about 8 ms |

Each card's compact and detail face repeats the same source reduction. Surface
construction across both discarded and retained faces totals 24 passes and
1416.0 ms; 1346.6 ms is inside the surface pixel loops. These are nested totals
and must not be added to the high-level table. The surface code performs 4x4
samples for framing across every covered pixel, including interiors. Initial
complete launcher rendering itself was **19.35 ms**; chrome **4.74 ms** and retained
buffer construction **4.28 ms**. All six card totals are similar (331.6–343.8 ms).
The measured delay is predominantly CPU work rather than a long blocking wait.

Evidence under ignored `build/magik-results/20260930T145043Z-e3548c3c2494/`:
`preparation-raw.json`, `profile.json`, `profile.folded`, `flamegraph.svg`.
Artifact SHA256:
`4e147dc04d1a2cb03e546d98ca3baede629f4ad34e7f62cc06770bd94c67a1b4`.
The prior uninstrumented 2020–2043 ms results remain the performance baseline;
this profile is not a new startup or 60 Hz qualification. The normal Dev launcher
was restored with `scripts/magik stop` (run `20260930T145203Z-4526f15a3707`).

The first optimisation candidates are to construct the RGB888 faces directly,
reuse each card's common source reduction for its two label variants, and reserve
subpixel framing work for pixels near actual boundaries. Their speedup and pixel
parity require new-implementation validation; the profile alone does not qualify
those changes or establish a sub-second first-latched-frame guarantee.
