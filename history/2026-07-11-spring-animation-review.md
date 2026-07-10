# Spring Animation Performance Review — 2026-07-11

## Scope

- Fixed point: `ce4f89f` (`Better animated launcher`).
- Scenario: 30-second Home `home-repeat-hold` at 960x540 RGB565.
- Spring implementation: analytic damped oscillator with persistent value,
  velocity, and target; Apple `Spring()` / `.smooth` defaults.

## Apple Default Parameters

Queried from the local SwiftUI framework on macOS 26:

- response / perceptual duration: 0.5s
- bounce: 0
- damping ratio: 1
- mass: 1
- stiffness: 157.91367041742973
- damping: 25.132741228718345
- estimated settling duration: 0.8s

## Paired CPU Comparison

The parent and reviewed builds used the same benchmark-enabled binary scope,
Home scenario, 30-second duration, and `fb0-dirty` backend solely to obtain a
paired CPU comparison. Both strict visual gates were invalid because fb0 wall
cadence includes scheduler/vsync jitter; neither run is visual evidence.

| Build | Frames | Work p99 | Work max | Work >16.667ms |
| --- | ---: | ---: | ---: | ---: |
| `SPRING-BASELINE-20260711` (`ce4f89f`) | 1,765 | 6,887us | 8,161us | 0 |
| `SPRING-REVIEWED-20260711` | 1,766 | 6,845us | 8,060us | 0 |

The reviewed spring path did not regress owned CPU work: p99 improved by 42us
(0.6%) and max improved by 101us (1.2%). The hot path is allocation-free. The
critical-damping path evaluates one exponential per active spring frame; derived
frequency and damping ratio are cached in `SpringConfiguration`.

## Authoritative Latch Gate

`SPRING-REVIEWED-LATCH-20260711` used the production
`fpga-vblank-latch-hidden` backend and passed (`valid=1`):

- 1,765 / 1,765 frames used latch, all status `ok`
- latch deadline misses: 0
- visual latch misses: 0
- buffer alternation failures: 0
- FPGA drop count: 0
- work >16.667ms: 0
- work p99: 6,863us; work max: 8,483us
- latch margin: p50 10,171us; p95 13,215us; p99 16,069us; min 8,097us
- latch copy: p50 1,280us; p95 1,461us; p99 1,500us

`fb0-dirty` visibly tore during the diagnostic comparison. It remains a
recovery fallback and is not valid for Home visual acceptance.
