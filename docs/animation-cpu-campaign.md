# Animation CPU campaign

Current base: `origin/main` at `d3dd4522b` (app-wide vsync-locked frame clock,
PR #216). Original hardware measurements used reporting PR #214 (`13a12fd9e`).
Work alone on
`nigel/animation-cpu-round-2`; reuse the managed reporting worktree and build cache.
Each numbered experiment is one commit followed by its relevant benchmark.

1. Version the complete 45-second SNES round trip and report moving-interval process CPU separately from mixed-window CPU.
2. Proven opaque-interior projected compose kernel.
3. Four/eight-column compose blocks with contiguous row stores.
4. Flat opaque-loop specialisation and spill reduction.
5. Reflection spill reduction.
6. Reuse exact per-pose projection setup.
7. Fuse ordinary filtering/shading while retaining intermediate floors.
8. Eliminate helper merge through explicit disjoint/coherent output ownership.
9. Sparse carousel/chrome damage for both hidden slots.
10. Noise-resistant bounded band balancing.
11. Clip device sampling to valid mapped source intervals.
12. Fuse panel base construction and union clear.
13. Narrow panel presentation damage with per-slot catch-up.
14. Bounded one-frame-ahead hierarchy trick rendering.
15. Animation-scoped helper real-time scheduling trial.
16. Separate UI real-time scheduling trial, including physical Main/input qualification.
17. Conservative near-edge-on projection bounds.

CPU/storage experiments use native Cortex-A9 kernel or renderer benchmarks plus
three complete unprofiled SNES round trips. Scheduling/buffering experiments add
controlled-contention, freshness and input/readiness measurements. Parent and
candidate must share build flags, affinity, assets, state and measurement contract.
Keep only repeatable improvements without pixel, cadence, input or readiness
regressions. Render-ahead cannot hide inadequate sustained throughput. Copies that
run after both bands complete do not by themselves justify a fixed helper bias.

Use `scripts/magik check animation-roundtrip --installed-sha256 SHA` for three
fresh native leases. The moving CPU metric reuses existing process CPU samples
between loop boundaries following the published UI-motion signal. It includes
background app threads and can lag motion boundaries by one loop; it is not an
exact per-kernel CPU measurement. Missing samples invalidate the average rather
than becoming zero CPU. Use separate diagnostic/profile runs for PMU, worker CPU
and scheduler attribution. Preserve exact scalar/ARM pixels, including rounded
caps, bilinear/mip boundaries, reversed faces, reflections and clipped tails.

Measurements and trial decisions live in ignored `outputs/animation-cpu-round-2`.
Do not stage raw logs, captures or benchmark artifacts. Publish a final comparison
once the campaign is complete; neither a lower microbenchmark cost nor fewer
cache events alone establishes 60 FPS with zero fresh-pose or physical drops.

The projected compose experiment includes a standalone native C fixture in
`crates/framebuffer-scenes/tests/launcher_column_bench.c`. Compile it as a
separate translation unit alongside `launcher_texture_neon.c`, using the same
`-O3 -std=c11 -mtune=cortex-a9 -mfpu=neon-vfpv3 -mfloat-abi=hard
-ffp-contract=off` flags as the production build. Run the resulting executable
through `MISTER_MAGIK2_PREBUILT_ARTIFACT=ABSOLUTE_PATH scripts/magik bench
card-column-generic` and `card-column-opaque`. Both workloads compare full output
before timing, then exercise a 32-column strip, four positive source strides and
four clipped source starts against the same 960x540 destination. Each reports
two wall-time and thread-CPU samples; repeat in reverse order to check drift.
This isolates compose and excludes filtering, reflections, copy and presentation.
Use the full application round trips to judge the effect on CPU and dropped frames.

Current decisions are recorded in `animation-cpu-results.md`. Projected opacity
and sparse hidden-slot copy are retained. Block stores, flat opacity and
filter/shade fusion were rejected. The reflection trial remains local and lacks
route qualification. The audit removed the unqualified helper scheduling
machinery and dedicated policy scenario; future scheduling work needs explicit
activation and Main/input evidence. No scheduler policy changes are currently
part of this branch.

Sparse copy preserves full seeding on each slot's content-generation change,
then copies the carousel and current chrome on every relevant frame. Full title
rows cover wider destination breadcrumbs and their clearing. Partial-copy
failure makes the slot unknown and forces complete reseeding before reuse.

After the frame-clock rebase, the native context records
`animation_clock.mode = vsync-locked-v1` and its period in nanoseconds. Route
steps retain a minimum two-second wall-time observation pause, then require
fresh metrics with `ui_motion = false` before the next input. A slow transition
must settle within the bounded route window; the host must not advance based
only on wall time. CPU, measurement windows and supervision deadlines still
use real elapsed time. These checks do not change motion or force extra renders.
Re-run parent and candidate under this same contract before comparing results
with the new app timing. Earlier wall-clock-animation figures are historical.

Use `scripts/magik check animation-roundtrip --profile --installed-sha256 SHA`
for one diagnostic route. This enables CPU sampling plus renderer stage spans
only inside the measurement window; ordinary runs do not enable span clocks.
The report includes helper frames and aggregates both bands' stage wall time.
Those totals are neither elapsed critical-path time nor stage CPU time. Compare
them with per-band CPU/run-delay, producer/merge, hidden-copy and actual Slint
raster evidence; profile-run drop counts are not acceptance measurements.


The first post-clock profile and clean comparisons are complete. Native retained
band publication (#8) removes the 0.715 ms helper merge without another buffer;
its clean routes reduced moving CPU by 2.76%. See `animation-cpu-results.md` for
all repetitions and the limited cadence conclusion. Next priority is bounded
render-ahead (#14), followed by clean sparse-copy and scoped-scheduler A/B. Keep
those separate from further kernels and from Slint/system pacing investigations.


Explicit reveal-stage profiling identified the scalar background fade and
sampling outside valid device coordinates. Separate commits now preserve exact
pixels while reducing those stage wall times by approximately 48% and 26%.
Clean route CPU is 18.82 ms/moving presentation, but drops remain about 21 per
route; this is CPU progress, not 60 FPS qualification. Cold transition starts,
endpoint Slint raster and card scheduling tails remain part of the full goal.
