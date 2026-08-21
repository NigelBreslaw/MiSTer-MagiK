# Cortex-A9 NEON Attribution

Date: 2026-08-22

## Outcome

The fixed `neon-attribution` campaign passed on installed Dev revision
`34b8dfb2e2f02d742a2e6f6ae47367b792a2415c`. All six profiles declared the
`cortex-a9-neon` counter set, every PMU group used the legacy ordered group-read
format required by this kernel, no PMU or profile records were dropped, and the
two production GUI legs retained zero physical dropped frames, latch drops, and
sequence gaps.

The most useful new signal is raw Cortex-A9 event `0x74` (NEON instructions).
Event `0x8c` (cycles with the NEON clock enabled) was approximately equal to CPU
cycles even in spans that executed zero NEON instructions. On this platform it
describes clock availability, not proof of vector work, so NEON clock duty must
not be used alone to attribute SIMD execution.

Evidence: [combined summary](../../build/agent-benchmarks/neon-attribution/1787351820/summary.json).

## Production GUI Results

| Metric | Landscape | Portrait left |
|---|---:|---:|
| Physical FPS | 59.946 | 59.949 |
| Physical / latch drops | 0 / 0 | 0 / 0 |
| Foreground p95 | 4,749 us | 6,921 us |
| PMU records | 27,911 | 27,905 |
| Profiled cycles | 6.047 billion | 9.955 billion |
| NEON instructions | 248.7 million | 428.4 million |
| NEON / speculative instructions | 10.12% | 12.54% |
| Data-dependent stall ratio | 29.86% | 38.57% |
| L1D refill ratio | 8.14% | 10.64% |

Portrait rendering does materially more work but remains cadence-safe. The
portrait leg accumulated 64.6% more profiled cycles and its foreground p95 was
45.7% higher. The counter movement identifies where that cost lives:

| Span | Mode | Cycles | NEON instructions | NEON share | Stall ratio | L1D refill ratio |
|---|---|---:|---:|---:|---:|---:|
| `gui.custom-layer-generation` | landscape | 1.031B | 4.63M | 0.85% | 26.31% | 7.14% |
| `gui.custom-layer-generation` | portrait | 4.999B | 228.32M | 15.18% | 44.74% | 11.09% |
| `gui.latch.arcade-overlay-copy` | landscape | 1.837B | 217.66M | 39.60% | 42.98% | 11.79% |
| `gui.latch.arcade-overlay-copy` | portrait | 1.679B | 164.90M | 35.31% | 44.34% | 15.04% |
| `gui.latch.preview-overlay-copy` | landscape | 0.208B | 16.44M | 29.29% | 38.18% | 13.36% |
| `gui.latch.preview-overlay-copy` | portrait | 0.387B | 33.45M | 29.96% | 43.48% | 13.85% |
| `gui.custom.preview-rotation` | portrait | 0.004B | 0.51M | 33.70% | 42.65% | 19.57% |

This is evidence that the intended RGB565 SIMD paths execute. It also changes
the optimization diagnosis: the major vectorized copy/rotation spans are
stall- and refill-heavy, so adding more NEON instructions is unlikely to be the
first win. Reducing bytes touched, duplicate full-plane passes, and cache-cold
source reads is the stronger next hypothesis.

Evidence: [landscape report](../../build/agent-benchmarks/neon-attribution/1787351820/arcade-landscape/report.md),
[portrait report](../../build/agent-benchmarks/neon-attribution/1787351820/arcade-portrait-left/report.md).

## Runtime Span Results

| Workload / span | Samples | Cycles | NEON instructions | NEON share | Stall ratio | L1D refill ratio |
|---|---:|---:|---:|---:|---:|---:|
| screensaver total | 720 | 1.134B | 12.30M | 1.90% | 38.74% | 7.36% |
| `screensaver.tile-blit` | 180 | 0.849B | 12.30M | 4.35% | 48.69% | 7.93% |
| search total | 264 | 0.703B | 1.37M | 0.34% | 8.05% | 1.24% |
| `search.sqlite` | 88 | 0.698B | 1.37M | 0.34% | 8.00% | 1.23% |

The screensaver result is cleanly concentrated: tile blitting accounts for
74.8% of its profiled cycles and essentially all measured NEON instructions.
Its 48.7% dependency-stall ratio makes memory/data flow the next question.
Search is almost entirely SQLite and shows negligible application-level NEON
share, so it is not a SIMD optimization target.

Catalog operations reported 0.20-0.45% aggregate NEON instruction share:

| Operation | Elapsed | Cycles | NEON instructions | NEON share | Stall ratio | L1D refill ratio |
|---|---:|---:|---:|---:|---:|---:|
| fresh build | 138.899 s | 104.273B | 276.45M | 0.448% | 22.32% | 2.68% |
| rebuild | 136.161 s | 67.374B | 90.52M | 0.203% | 16.89% | 1.84% |
| rebuild all | 114.535 s | 91.273B | 261.21M | 0.442% | 15.67% | 2.12% |

No catalog phase meets the repository's SIMD-candidate threshold. The largest
NEON total was `catalog.persist` at 232.34M instructions, but that was only
0.74% of speculative instructions over 54.694B cycles. Catalog optimization
should remain focused on filesystem traversal, publication bytes, and SQLite
work rather than new hand-written NEON.

Evidence: [screensaver](../../build/agent-benchmarks/neon-attribution/1787351820/runtime-spans/screensaver.json),
[search](../../build/agent-benchmarks/neon-attribution/1787351820/runtime-spans/search.json),
[catalog](../../build/agent-benchmarks/neon-attribution/1787351820/runtime-spans/catalog.json).

## Implementation Checklist

- [x] Add a selectable seven-event Cortex-A9 NEON group without multiplexing.
- [x] Store only events actually measured and retain counter-set provenance.
- [x] Surface raw values and derived per-span metrics in v2 artifacts.
- [x] Configure gatord with the matching Cortex-A9 counter list.
- [x] Guard optional `sl-analyze` behind an explicit absolute path, real output
  directory, retained logs, and a 120-second timeout.
- [x] Add a fixed runtime, landscape, and portrait attribution campaign.
- [x] Deliver the exact clean runtime and pass device smoke validation.
- [x] Run the full campaign and retain all structured evidence.

`sl-analyze` was not invoked during this campaign. The earlier crash was limited
to an exploratory `--no-output` invocation; the committed workflow never uses
that mode and never launches the Streamline GUI.

Implementation commits: `6d96384ad`, `30d2d2c00`, `9f5229785`, `4b7499741`,
and portability fix `34b8dfb2e`.
