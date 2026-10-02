# Animation CPU campaign

Base: merged reporting PR #214 (`13a12fd9e`). Work alone on
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
