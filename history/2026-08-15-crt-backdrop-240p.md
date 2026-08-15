# CRT screenshot backdrop prototype — 240p — 2026-08-15

## Scope

This is the requested 240p-only device trial. The device was left on
`crt-240p60`; 288p was not selected or measured. The implementation also
contains the shared 288p route support, but no 288p qualification claim is
made here.

## Committed implementation

- Source commit: `001f3483` (`perf(agent): qualify Arcade scrolling at 50 and 60 Hz`)
- Logical commits: five runtime commits, covering the 5% low-resolution safe
  area, RGB565 backdrop primitives, preview composition, production CRT
  routing, and route-aware benchmark accounting.
- Platform release: `platform-v0.28`
- Platform bundle: `fca9ad824df5c630e6252d0c6f445b8c3afe2fa568d671f9aead40f0593a41fe`
- Installed GUI revision: `001f3483797142fbb280aea5e404dc139c9f0181`

## Route and visual parameters

- Route: `crt-240p60`, 640×240 physical framebuffer, 640×480 logical
  composition, direct video enabled.
- Safe content rectangle: 32 px left/right and 24 px top/bottom in the
  logical 240p composition, preserving the 5% overscan margin in physical
  raster space.
- Backdrop: center-cropped 4:3, nearest-neighbour RGB565, 40% channel retain
  (60% black), 130 ms crossfade.
- Capture: [terminal-arcade.png](../../build/agent-benchmarks/arcade-velocity-scroll/1786825624/terminal-arcade.png)
- Capture metadata: [terminal-arcade.json](../../build/agent-benchmarks/arcade-velocity-scroll/1786825624/terminal-arcade.json)

Visual inspection of the authoritative scanout shows the screenshot reaching
the raster edges, while the header, list, footer, separators, selected row,
and readable text remain inside the safe rectangle. The selected cyan row stays
legible over the darkened image.

## Authoritative 20-second Arcade hold

Evidence directory:
[1786825624](../../build/agent-benchmarks/arcade-velocity-scroll/1786825624/)

| Metric | Result |
| --- | ---: |
| Hold | 20,987,554 µs |
| Submitted/profile frames | 664 |
| Physical refresh | 60.038 Hz |
| Minimum required | 59.9 Hz |
| Physical repeated/dropped frames | 595 |
| Latch drops | 0 |
| Ownership losses | 0 |
| Sequence gaps | 0 |
| Selection changes | 67 |
| Foreground p95 / p99 / max | 35,296 / 39,531 / 44,253 µs |
| Backdrop preparation p95 / p99 / max | 4,900 / 5,075 / 5,962 µs |
| Backdrop blend p95 / p99 / max | 9,298 / 9,849 / 11,274 µs |

The run failed the prototype gates for physical cadence, physical repeated
frames, and the 13,300 µs foreground p99 budget. The latch protocol itself
remained coherent (zero latch drops, ownership losses, and sequence gaps).
The result is retained as prototype evidence and must not be described as
qualified.

## Follow-up

The remaining bottleneck is full-frame crossfade/composition on the ARM render
path, not screenshot decoding or route geometry. Further work should move
backdrop blend/composition off the critical render path or reduce its work
while retaining the 130 ms visual result. No 288p device trial was performed.
