# Settled composition attribution — 2026-08-22

## Authority

- Host revision: `c5da15d28`
- Installed MagiK revision: `2ce1b4d50ed1b8f3d9ebe0f605a6b30db35f8570`
- Installed Main revision: `639d3694e1b93660020e9587cd0fe27f0170ce4c`
- Display: `hdmi-1280x720p60`
- Performance authority: unprofiled installed Dev runtime
- Artifact: `build/agent-benchmarks/settled-composition/1787370984/summary.json`

The fixed route held the real Arcade favourite confirmation without accepting
it, returned Home, entered Settings, captured the forced destination frame and
its immediate successor, then restored Home with the Arcade tile selected.

## Results

The modal reached `modal-over-arcade`, direct layers were retired, and all 51
steady frames carried the same authoritative modal retirement receipt. Despite
that confirmed retirement, current production behavior forced a full present on
all 51 frames:

- steady full presents: 51/51
- steady copied bytes: 94,003,200 (1,843,200 per frame)
- steady Slint raster: 6,385us total
- steady custom-layer work: 1,060us total

The Home-to-Settings destination was a forced full Slint raster:

- destination frame: 116
- destination Slint raster: 13,132us
- destination custom-layer work: 3,956us
- destination copy bytes: 0
- immediately following frame: 117
- following-frame Slint raster: none
- two-frame Slint raster: 13,132us
- two-frame copy bytes: 0

Physical presentation qualification passed with zero repeated vblanks, latch
drops, ownership losses, sequence gaps, or phase outliers during the measured
window. Installed binary, Main, latch RBF, and scanout-module identities were
stable for the route.

## Disposition

Item 8 is qualified. The result establishes a direct Item 9 target: stop
steady modal carrier presents only after the matching physical retirement
receipt is authoritative, while retaining the entry carrier and uncertainty
recovery path. Item 10 remains independently measurable from the 13.132ms
Settings destination raster and its zero-raster successor.
