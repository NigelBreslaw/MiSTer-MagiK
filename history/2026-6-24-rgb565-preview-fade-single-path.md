# RGB565 Preview Fade Single Path

Item 5 of the production performance plan.

## Confirmed Cause

The production fade path still carried a same-geometry fast path plus a generic
RGB565 fade fallback. CPU profile on the immediate parent showed the fallback
dominating the preview scroll hot path:

- `ITEM05-BEFORE-fade-unify-cpu`: `blit_transition_565_fade_generic` was
  541 samples / 32.73% in the flamegraph.
- `preview_blit_us` p99 was 3313 us in the production 60 second run and
  3830 us in the CPU-profiled run.

## Fix

The fade now uses one RGB565 production path:

- `blit_transition_565_fade` always routes fade work through
  `blit_transition_565_fade_rows`.
- The path handles clipping, centered offsets, scaled RGB565 frames, same-size
  frames, and empty sides in the same row/segment routine.
- Same-size rows use the same path with a row blend micro-kernel.
- Scaled rows sample through the same path without reintroducing the old RGB
  fallback.
- Optimized ARM builds now enable `-C target-feature=+neon`, so the row blend
  uses Rust ARM NEON intrinsics under the same cfg pattern as the framebuffer
  copy code.

No C helper is used. The earlier diagnostic C helper detour was removed.

## Benchmarks

Immediate-parent before evidence:

```text
scripts/profile-preview-scroll.sh 60 turbo-hold ITEM05-BEFORE-fade-unify --skip-build --visual-captures 0
scripts/profile-preview-scroll.sh 60 turbo-hold ITEM05-BEFORE-fade-unify-cpu --skip-build --cpu-profile --visual-captures 0
```

Final candidate after evidence:

```text
scripts/deploy-rust.sh --device --ui-scope launcher
scripts/profile-preview-scroll.sh 60 turbo-hold ITEM05-AFTER-rust-neon-single-path --skip-build --visual-captures 0
scripts/profile-preview-scroll.sh 60 turbo-hold ITEM05-AFTER-rust-neon-single-path-cpu --skip-build --cpu-profile --visual-captures 0
```

| Run | Artifact | `preview_blit_us` p99 | Result |
| --- | --- | ---: | --- |
| Before | `build/preview-scroll-profiles/ITEM05-BEFORE-fade-unify-arcade.tsv` | 3313 us | baseline |
| After | `build/preview-scroll-profiles/ITEM05-AFTER-rust-neon-single-path-arcade.tsv` | 1655 us | 50.05% reduction |
| Before CPU | `build/preview-scroll-profiles/ITEM05-BEFORE-fade-unify-cpu-arcade.tsv` | 3830 us | baseline |
| After CPU | `build/preview-scroll-profiles/ITEM05-AFTER-rust-neon-single-path-cpu-arcade.tsv` | 1718 us | 55.14% reduction |

The absolute p99 target of 1500 us was not reached, but the plan's alternate
success metric, at least 50% reduction, was reached on both the production and
CPU-profiled runs.

CPU SVG checks:

- Before: `build/preview-scroll-profiles/ITEM05-BEFORE-fade-unify-cpu-arcade-cpu.svg`
  contains `blit_transition_565_fade_generic`.
- After: `build/preview-scroll-profiles/ITEM05-AFTER-rust-neon-single-path-cpu-arcade-cpu.svg`
  contains the single fade chain
  `blit_transition_565_fade -> blit_transition_565_fade_rows -> blend_565_row`
  and no `blit_transition_565_fade_generic` or
  `blit_transition_565_fade_same_geometry` symbols.

## Build-Flag Check

The Rust NEON path follows the existing project pattern from
`magik-gui/src/framebuffer_copy.rs`: compile the intrinsic code only when
`target_arch = "arm"` and `target_feature = "neon"` are both true. The
optimized ARM build scripts enable that target feature.

Stable `rustc 1.96.0` accepts a direct metadata probe of the relevant intrinsics
with `-C target-feature=+neon`; it emits the known command-line warning:

```text
warning: unstable feature specified for `-Ctarget-feature`: `neon`
```

The same warning appears in ARM builds and does not fail the build or the clippy
gate. It matches the older `experiments/neon-cross-rust` history.

## Review

Two subagent reviews were run before commit:

- Correctness review found no clipping, offset, scaled-frame, alpha endpoint,
  segment, or destination-bounds blocker for valid RGB565 frames. It noted that
  non-RGB565 production transition frames now render as absent; this is accepted
  for the production RGB565-only scope.
- Evidence review confirmed the before/after reduction, the CPU flamegraph
  symbol removal, and the build-flag caveat above.

## Validation

```text
cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features rgb565
scripts/test-host-tools.sh
cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings
cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings
```
