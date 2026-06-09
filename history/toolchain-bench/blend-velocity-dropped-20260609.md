# Dropped Blend Velocity Experiments - 2026-06-09

## PR 3 Candidate: Force Existing u32x4 NEON Intrinsics

Rejected before benchmark.

Baseline: PR2 NEON-target scalar/autovectorized path:

- Label: `BLEND-PR2-NEON`
- Runtime backend label: `blend_backend=scalar`
- `fade_blend_us` p50/p95: `702` / `736`
- `fade_copy_us` p50/p95: `220` / `257`
- `body_copy_us` p50/p95: `770` / `804`

Attempt:

- Changed cfgs to compile and call `blend_row_towards_neon` on all arm builds.

Result:

- `scripts/profile-blend-velocity.sh 15 BLEND-PR3-U32X4 real-text --deploy-fast`
  failed during cross-build.
- Stable Rust rejected `core::arch::arm` NEON intrinsics with
  `E0658: use of unstable library feature stdarch_arm_neon_intrinsics`.
- No device benchmark was produced.

Decision:

- Do not use direct `core::arch::arm` intrinsics on this stable toolchain.
- Keep `-C target-feature=+neon` and the autovectorized scalar loop as the active
  path.
- Future NEON experiments should use stable-friendly approaches: scalar loop
  shapes that autovectorize better, assembly inspection, or a separate carefully
  gated assembly/C helper if the benchmark still justifies it.

## PR 5 Candidate: Cache Fully Blended Fade Rows

Rejected after benchmark.

Baseline: PR4 precomputed fade constants:

- Label: `BLEND-PR4-FINAL`
- `fade_blend_us` p50/p95: `653` / `686`
- `fade_copy_us` p50/p95: `227` / `266`
- `body_copy_us` p50/p95: `768` / `796`

Attempt:

- Added a small in-memory ring cache for blended fade rows in the standalone
  `blend_velocity` benchmark.
- Cache key included physical source row, source row version, and fade alpha
  index.
- Instrumented per-frame cache hits and misses.

Result:

- Label: `BLEND-PR5-CACHE`
- `fade_blend_us` p50/p95 regressed to `1078` / `1148`.
- `fade_copy_us` p50/p95 regressed to `242` / `287`.
- Cache hit rate was `0 / 96` rows per frame; every fade row missed.
- Binary size grew by about `8 KiB`.

Decision:

- Drop the fully blended row cache for 6 px/frame velocity scrolling.
- The source row plus alpha combination does not repeat under this motion pattern,
  so cache lookup and storage only add overhead.
- Future caching should only target a more stable identity, such as rendered text
  spans or background bands, not fully blended moving rows.
