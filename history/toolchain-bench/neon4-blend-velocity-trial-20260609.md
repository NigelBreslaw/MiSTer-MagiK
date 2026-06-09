# NEON4 Blend Velocity Trial

Date: 2026-06-09

## Target

Standalone arcade text scrolling list with fade:

```bash
scripts/profile-blend-velocity.sh 15 <LABEL> real-text
```

The acceptance metric was `fade_blend_us` on `real-text`; the synthetic
`baseline` variant was used as a lower-noise confirmation.

## Current Baseline

Current committed/stable build:

| Label | Variant | Backend | fade_blend_us p50 | fade_blend_us p95 | wall_us p50 |
|---|---|---|---:|---:|---:|
| `NEON4-BASE-REALTEXT` | `real-text` | scalar | 647 | 686 | 16490 |
| `NEON4-BASE-BASELINE` | `baseline` | scalar | 632 | 667 | 16502 |

## Experiments

Trial build used the pure-Rust NEON approach from `experiments/neon-cross-rust`,
but only as a temporary benchmark build:

- nightly Rust
- local `#[target_feature(enable = "neon")]`
- no global `+neon`, to avoid dependency failures in crates such as
  `simd-adler32`

Four NEON variants were tested against `real-text`:

| Label | Backend | Shape | fade_blend_us p50 | fade_blend_us p95 | Result |
|---|---|---|---:|---:|---|
| `NEON4-U32RGB` | `u32rgb` | separate `r/g/b` lanes | 676 | 717 | slower |
| `NEON4-U32RB` | `u32rb` | paired red/blue plus green | 642 | 659 | small win |
| `NEON4-U32RGB2` | `u32rgb2` | unrolled `u32rgb` | 672 | 689 | slower |
| `NEON4-U32RB2` | `u32rb2` | unrolled `u32rb` | 598 | 638 | best |

Winner confirmation on synthetic `baseline`:

| Label | Variant | Backend | fade_blend_us p50 | fade_blend_us p95 | wall_us p50 |
|---|---|---|---:|---:|---:|
| `NEON4-BASE-BASELINE` | `baseline` | scalar | 632 | 667 | 16502 |
| `NEON4-U32RB2-BASELINE` | `baseline` | `u32rb2` | 613 | 639 | 16470 |

## Notes

The benchmark wrapper initially failed to forward `MISTER_BLEND_NEON` to the
device. The accidental local-only scalar run (`NEON4-U32RGB` before rerun)
reported `blend_backend=scalar` and `fade_blend_us p50=1409`, showing that
removing the normal global `+neon` build flag without enabling a replacement
backend is very bad for this loop.

## Decision

Do not leave the code trial in production yet.

`u32rb2` is a promising result for the fade loop:

- `real-text` p50 improved from `647us` to `598us` (~7.6%)
- `real-text` p95 improved from `686us` to `638us` (~7.0%)
- synthetic `baseline` p50 improved from `632us` to `613us` (~3.0%)

However, the pure-Rust path currently requires nightly-only ARM feature gates and
a local-target-feature build mode. It should be ported as a focused PR only if we
are willing to make that toolchain trade-off explicit, or converted into a small
isolated helper that does not disturb the normal stable build.

