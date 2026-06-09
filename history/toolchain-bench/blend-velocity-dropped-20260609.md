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
