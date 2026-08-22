# Custom-layer damage intersection qualification

Date: 2026-08-22

## Scope

This qualification covers roadmap Item 11 and the proceed gate for Item 12.
Production bounding-box invalidation remained authoritative throughout the
measurement. The counterfactual per-rectangle classifier was diagnostic only.

Installed device revisions:

- MiSTer MagiK: `dbc2ba68055755a5675f0e62fa3dbfc35b899b82`
- Main_MiSTer: `639d3694e1b93660020e9587cd0fe27f0170ce4c`
- Host benchmark driver recovery: `69c50a77c38b417b40326edf8779231f473bdc52`

## Implementation checklist

- [x] Computed bounding-box and per-rectangle Arcade intersections side by side.
- [x] Computed bounding-box and per-rectangle preview intersections side by side.
- [x] Kept bounding-box behavior authoritative during every measurement.
- [x] Counted Arcade and preview false-positive invalidations independently.
- [x] Retained copied-byte, custom-layer, Slint-damage, and cadence telemetry.
- [x] Exercised the real Arcade alphabet/filter drawer open and close route.
- [x] Exercised disjoint Slint chrome damage in the Settings destination route.
- [x] Ran fixed GUI-frame, Settings, landscape Arcade, and portrait Arcade controls.

## Benchmark-driver recovery

The first real-drawer run reached `drawer_level=Filters` but timed out with the
drawer still open. The dominant failed phase was benchmark navigation: Left
opened the alphabet drawer and one B returned only to the top Filters level.
The bounded recovery waits for that intermediate semantic state, then activates
the already-selected Games row to close the drawer without leaving Arcade.
No production launcher behavior changed.

- Failed run: `build/agent-benchmarks/settled-composition/1787373891/launcher.log`
- Recovered run: `build/agent-benchmarks/settled-composition/1787374055/summary.json`

## Exact-device results

### GUI frame attribution

Artifact: `build/agent-benchmarks/gui-frame-attribution/1787373451/summary.json`

- Control frames: 117
- Arcade bounding/per-rectangle invalidations: 0 / 0
- Preview bounding/per-rectangle invalidations: 0 / 0
- Arcade/preview false positives: 0 / 0
- Latch drops, sequence gaps, ownership losses, repeated physical vblanks: all 0

### Real Arcade drawer and settled composition

Artifact: `build/agent-benchmarks/settled-composition/1787374055/summary.json`

- Profile frames: 118
- Custom-damage phase frames: 35
- Frames with disjoint Slint rectangles: 1
- Disjoint frames while Arcade bounding invalidation was active: 0
- Arcade bounding/per-rectangle invalidations: 1 / 1
- Preview bounding/per-rectangle invalidations: 1 / 1
- Arcade/preview false positives: 0 / 0
- Total measured custom-layer work: 191,924 us
- Total copied bytes: 34,953,536
- Latch drops, sequence gaps, ownership losses, repeated physical vblanks: all 0
- Terminal Settings PNG SHA-256:
  `42e7ef05f7510300246df562e67c54c9de481cf6abc70651db0917e40eabd58c`

The drawer open/close work regenerated real custom layers but produced no Slint
damage rectangles. The only disjoint Slint damage occurred after composition
had become full-Slint, where neither Arcade nor preview could be spuriously
invalidated.

### Settings landscape and portrait control

Artifact: `build/agent-benchmarks/settings-navigation/1787374140/summary.json`

- Qualification: passed, 12/12 directed legs
- Maximum whole-frame work: 6,260 us
- Portrait maximum whole-frame work: 4,920 us
- Physical drops, latch drops, sequence gaps, ownership losses: all 0

### Arcade landscape control

Artifact: `build/agent-benchmarks/arcade-velocity-scroll/1787374220/summary.json`

- Frames: 2,458 over the fixed 40-second hold
- Physical FPS: 59.998886
- Foreground work p95 / p99 / max: 4,279 / 5,238 / 6,024 us
- Physical drops, latch drops, sequence gaps, ownership losses: all 0

### Arcade portrait-left control

Artifact:
`build/agent-benchmarks/arcade-velocity-scroll-attribution/1787374291/summary.json`

- Authority: unprofiled qualification control
- Frames: 2,458 over the fixed 40-second hold
- Physical FPS: 60.000531
- Foreground work p95 / p99 / max: 7,722 / 9,854 / 12,041 us
- Physical drops, latch drops, sequence gaps, ownership losses: all 0

## Gate disposition

Item 12 requires real false-positive invalidations before an experiment is
opened. The measured count is zero for both custom layers, so the proceed gate
failed with no optimization opportunity. No `experiment(ui)` selector or
production path was added. Multi-rectangle custom invalidation remains closed
until a production route demonstrates a nonzero false-positive count.
