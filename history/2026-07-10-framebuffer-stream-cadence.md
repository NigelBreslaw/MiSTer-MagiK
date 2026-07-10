# Framebuffer Stream Cadence And Rollout Decision

Date: 2026-07-10

## Outcome

Keep production framebuffer streaming `off`.

The cadence instrumentation, deterministic desktop probes, macOS display-link
controller, 1Hz Analytics chrome, formal gate, resolution profiles, and MiSTer
scalar/NEON benchmark are implemented. The work found three independent failed
rollout conditions:

1. Slint accepts the rendering notifier but emits no `AfterRendering` events in
   the compiled Skia Analytics window, so there is no formal presentation
   evidence.
2. Half-frame producer snapshots miss the 4ms/6ms p95/max gate because copying
   the full immutable hidden slot dominates the now-fast decimator.
3. Explicit Cortex-A9 NEON is slower than the truthful scalar decimator and
   fails the required 1.5x speedup.

The adaptive refinement probe also produced no refinement because the current
`human-turbo-hold` scenario pauses only near startup, before the consumer is
reliably attached, and then moves continuously. That profile is not yet a valid
motion-to-settle proof.

## Desktop Cadence

The desktop now consumes at most one newest frame per native macOS
`CADisplayLink` tick. Producer publication only replaces the one-slot mailbox;
Winit redraw events are informational. Analytics chrome refreshes from a
separate 1Hz Slint timer.

Release synthetic probes showed the source near 60fps and application near
56fps with roughly 6% coalescing. The display clock ran at the monitor's native
roughly-120Hz cadence. However, all Slint notifier event counts remained zero,
including `AfterRendering`, despite successful notifier registration. Formal
runs therefore fail with `invalid_reason=zero_after_rendering` rather than
miscounting receive, apply, or redraw submission as presentation.

## Sustained Producer And Transport Measurements

Matched null-drain evidence was generated under `build/arcade-scroll-profiles/`
and is intentionally not committed as raw benchmark output.

| Profile | Transport fps | Interval p95 | Raw bytes | Average payload | Snapshot p95/max | Producer coalesced |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| half | 59.90 | 24.4ms | 259,200 | 30,943 | 9.413/12.020ms | 0 |
| full | 56.01 | 89.3ms | 1,036,800 | 96,934 | 8.877/11.585ms | 110 |
| adaptive motion | 59.67 | 24.0ms | 259,200 | 31,111 | 9.672/12.951ms | 0 |

The full profile proves that sustained 960x540 transport is about 56fps on this
scene, not 60fps, and its long interval tail matches the previously visible
pulse. Half/adaptive motion reaches the required average transport rate but
still misses the producer snapshot gate and the stricter cadence conditions.

The adaptive run reported `adaptive_refinements=0`; no exact 960x540 refinement
was observed, so the 15ms refinement requirement remains unproved.

## RGB565 Scalar Versus NEON

Commit `2b4b990` adds a bench-tools command and device gate. Stable Rust accepts
the ARM `+neon` code-generation flag but does not expose
`cfg(target_feature = "neon")` for this target, and ARM NEON intrinsics remain
unstable. The fixed MiSTer build therefore compiles a small C helper with
`-mfpu=neon`; the original Rust intrinsic guard remains available for a
toolchain that exposes it.

The device command compares scalar and NEON outputs for contiguous 960x540,
padded 960x540, and odd 959x539 inputs. All checksums matched exactly. The final
960x540 result was:

| Implementation | p50 | p95 | max |
| --- | ---: | ---: | ---: |
| scalar | 0.931ms | 1.011ms | 1.082ms |
| NEON | 1.322ms | 1.476ms | 1.630ms |

The reported speedup was `0.685x`: NEON was slower. This decimator keeps one
16-bit pixel from every pair; scalar can load only the kept halfword, while the
vector kernel pulls both pixels before narrowing. Both implementations meet the
absolute 4ms/6ms limit, but NEON fails the 1.5x requirement decisively.

`MISTER_FRAMEBUFFER_STREAM_SIMD=auto` therefore selects the faster fixed-target
scalar implementation. Explicit `neon` remains available for measurement. The
same stable-Rust cfg limitation applies to the existing screenshot-fade Rust
NEON guards, so those paths must not be assumed active without their own runtime
backend evidence.

## Decision

Do not create the planned default-on commit. Leave unset stream scale as `off`.
Before reconsidering adaptive-by-default:

- obtain distinct presentation evidence from Slint or another non-invasive
  macOS completion signal;
- move or avoid the full hidden-slot copy so half snapshots meet 4ms/6ms;
- make the refinement scenario perform motion and then a measured late pause;
- rerun the automated 30-second display-link gate;
- complete one attended 30-second no-pulse check.
