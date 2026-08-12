# Deep profiling optimization shortlist, 2026-08-12

This list applies the campaign's decision gates to the exact baseline in
`history/2026-08-12-deep-profiling-baseline.md`. It recommends subsequent
investigations; it does not prescribe or implement a code change.

| Rank | Candidate | Measured opportunity | Confidence | User impact | Implementation risk | Decision |
| ---: | --- | --- | --- | --- | --- | --- |
| 1 | Navigation and orientation custom raster | navigation raster was 174.5M cycles, 28.9% of the GUI route; orientation frames reached 25.8-37.3 ms and repeated physical vblanks | high | very high | medium-high | advance; profile the raster's memory traversal and transformed-write subphases before designing a change |
| 2 | Hidden-slot damage/catch-up copy amplification | 10.10x copied bytes per Slint damage byte; named base, Arcade, catch-up, and preview copies totalled 74.5M cycles | high | very high | high | advance; distinguish unavoidable stale-slot restoration from avoidable invalidation and overlay copying |
| 3 | Continuous framebuffer observer interference | adaptive/full observers added 89.8/101.2 ms to the same Home-pan route | high | high for Analytics live view | medium | advance; use Streamline to separate GUI producer, agent proxy, TCP, and desktop consumer costs before choosing an owner |
| 4 | Legacy PNG capture pipeline | high-entropy capture took 188-197 ms; RGB conversion plus zlib consumed 140-150 ms and 5.81 MB of peak buffer ownership | high | medium | medium | advance; evaluate whether the legacy typed PNG path is still required before optimizing its conversion or encoding |
| 5 | Cold V1 directory enumeration | first 961-entry listing spent 1.411 s enumerating versus 46 ms warm; V2 completed in about 50 ms | high | medium | low-medium | advance if V1 remains user-facing; first confirm real clients cannot use V2 |
| 6 | Telemetry process discovery | 100 ms arms spent 1.435-1.721 s in discovery across 25-30 samples, roughly 48-57 ms per sample | medium | medium for profiling fidelity, low in the default 1 Hz mode | low-medium | advance as observer-tax work; require a longer unprofiled control before claiming GUI cadence benefit |
| 7 | System-entry publication and first-frame preparation | first-frame preparation was 7.1-8.6 ms; destination publication was 13-32 ms while catalog open itself stayed below 0.8 ms | medium | medium | medium | keep behind ranks 1-6; the complete C64/SNES paths already pass at 56/86 ms |

## Deferred or blocked

- Launch/return remains blocked on route validity: two bounded control attempts
  failed before a return capsule was written. No optimization candidate can be
  selected until the fixed route completes and produces control, PMU, pprof,
  and Streamline arms.
- Reverse portrait Settings evidence is incomplete. Fixing evidence production
  is a profiling-contract task, not a performance optimization.
- Catalog V3 intentionally has no `library.sqlite3`; snapshot optimization is
  not applicable and the retired file must not be recreated.
- Bridge synchronization, timer dispatch, store publication, and preview-copy
  spans did not independently cross the campaign's 10% or 1 ms interactive
  gate.

## Required next evidence

Each advanced candidate should receive its own optimization plan and an
unprofiled before/after authority run. A change advances only if it preserves
zero latch drops, sequence gaps, and ownership loss and removes the associated
physical repeated vblanks. PMU and Streamline deltas remain explanatory, not
qualification evidence.
