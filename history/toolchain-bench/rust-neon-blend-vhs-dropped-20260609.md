# Dropped Experiment: Pure Rust NEON For Blend + VHS

Date: 2026-06-09

## Summary

Tried the new pure-Rust NEON approach from `experiments/neon-cross-rust` against:

- `blend_velocity` fade blending
- `vhs_glitch` / CrtImage normal-row color scaling

The experiment proved the Rust NEON path can be made active in the main binary,
but it did not improve the measured MiSTer benchmarks. The code trial was
dropped.

## Toolchain Findings

The standalone experiment works by using nightly Rust with:

- `#![feature(stdarch_arm_neon_intrinsics)]`
- `-C target-cpu=cortex-a9 -C target-feature=+neon`

In the full app, global `+neon` on nightly caused dependency fallout:
`simd-adler32` compiled its own ARM NEON module and failed because dependency
crates cannot inherit our crate-level feature gate.

The workable main-app trial was:

- nightly Rust
- no global `+neon`
- local `#[target_feature(enable = "neon")]` on our unsafe functions
- `#![feature(arm_target_feature, stdarch_arm_neon_intrinsics)]`

That produced a deployed binary whose blend benchmark reported:

```text
blend_backend=rust-neon-u32x4
```

## Benchmarks

Blend velocity, `real-text`, 15s:

| Build | Backend | fade_blend_us p50 | fade_blend_us p95 | wall_us p50 |
|---|---:|---:|---:|---:|
| `NEONRUST-BASE-REALTEXT` | scalar | 650 | 684 | 16465 |
| `NEONRUST-AFTER-REALTEXT` | rust-neon-u32x4 | 654 | 699 | 16487 |

Blend velocity, `baseline`, 15s:

| Build | Backend | fade_blend_us p50 | fade_blend_us p95 | wall_us p50 |
|---|---:|---:|---:|---:|
| `NEONRUST-BASE-BASELINE` | scalar | 653 | 683 | 16478 |
| `NEONRUST-AFTER-BASELINE` | rust-neon-u32x4 | 659 | 694 | 16486 |

VHS/CrtImage:

| Build | Mode | effect_us | scale_copy_us | fps |
|---|---:|---:|---:|---:|
| `NEONRUST-BASE-VHS-NATIVE` | `320x224 native` | 1016 | 376 | 60.0 |
| `NEONRUST-AFTER-VHS-NATIVE` | `320x224 native` | 1040 | 383 | 60.0 |
| `NEONRUST-BASE-VHS-2X` | `320x224 fill=2x` | 1021 | 1064 | 60.0 |
| `NEONRUST-AFTER-VHS-2X` | `320x224 fill=2x` | 1038 | 1101 | 60.0 |
| `NEONRUST-BASE-VHS-640` | `640x448 native` | 60258 | 1609 | 16.2 |
| `NEONRUST-AFTER-VHS-640` | `640x448 native` | 63544 | 1657 | 15.3 |

## Decision

Drop.

The active Rust NEON `u32x4` loops were slightly slower than the scalar code in
both target benchmarks. The local-only build also disabled the existing global
`target_feature=neon` cfg path, which made 2x copy slightly worse.

Future NEON work should either:

- use a different loop shape, especially narrower/widening channel lanes instead
  of `u32x4`, or
- avoid nightly/global toolchain churn and use a focused C helper if explicit
  intrinsics are still worth testing.

