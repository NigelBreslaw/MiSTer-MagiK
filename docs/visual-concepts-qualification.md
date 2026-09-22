# Mini visual concepts — device qualification

Status: implementation available; qualification is incomplete. The curved tunnel missed its physical cadence gate; the user deferred further tunnel tuning. Live HDMI visual acceptance remains with the user.

These are the latest completed measurements for each concept, across the explicitly identified development binaries below. A prior binary pass does not qualify a changed renderer.

Observed mode: HDMI 1920×1080p60; resolved RGB565 rendering: 960×540. No display-mode changes. Each row uses the default preset, two unprofiled 30-second device-clock windows and two-second warmups. Captures and playback checks run afterward. Streaming publication is disabled during measurement.

| Concept | FPS W1 / W2 | Samples W1 / W2 | Repeats W1 / W2 | CPU % W1 / W2 | Peak RSS MiB | Device gate |
|---|---:|---:|---:|---:|---:|---|
| point-cloud-morph | 59.998 / 60.000 | 1800 / 1800 | 0 / 0 | 102.58 / 102.58 | 34.3 | Pass |
| depth-parallax | 60.000 / 59.998 | 1800 / 1800 | 0 / 0 | 100.17 / 100.26 | 76.6 | Pass |
| light-sweep | 59.998 / 60.000 | 1800 / 1800 | 0 / 0 | 100.16 / 100.17 | 34.2 | Pass |
| mirror-floor | 60.000 / 60.000 | 1800 / 1800 | 0 / 0 | 100.17 / 100.17 | 65.3 | Pass |
| pixel-dissolve | 59.998 / 59.998 | 1800 / 1800 | 0 / 0 | 100.15 / 100.23 | 65.3 | Pass |
| starfield-comets | 60.000 / 59.998 | 1800 / 1800 | 0 / 0 | 100.13 / 100.12 | 65.3 | Pass |
| palette-aurora | 60.000 / 59.998 | 1800 / 1800 | 0 / 0 | 100.16 / 100.16 | 34.2 | Pass |
| texture-tunnel | 59.998 / 59.898 | 1800 / 1797 | 0 / 3 | 100.28 / 100.20 | 56.5 | Fail |
| raster-waves | 60.000 / 60.000 | 1800 / 1800 | 0 / 0 | 100.17 / 100.23 | 34.0 | Pass |
| wireframe-terrain | 60.000 / 59.998 | 1800 / 1800 | 0 / 0 | 100.17 / 100.17 | 34.0 | Pass |

Required gates: zero protocol-v5 physical repeats, zero latch drops/rejections, valid ownership, one advanced timeline frame per presentation, CPU below 150% in each window, process peak RSS at most 128 MiB. Actual refresh comes from owned-vblank deltas rather than assuming exactly 60 Hz.

## Timings and evidence

Times below are the larger average of the two windows, in milliseconds. Frame-to-present includes rendering, transfer and settled latch wait; p99 is the larger render p99. Raw logs retain each window and maxima.

| Concept | Render avg | Transfer avg | Frame-to-present avg | Render p99 | Ignored evidence directory |
|---|---:|---:|---:|---:|---|
| point-cloud-morph | 1.555 | 1.459 | 16.548 | 2.917 | `build/magik-results/20260922T140623Z-4592d6922fd3` |
| depth-parallax | 6.625 | 0.968 | 16.559 | 12.020 | `build/magik-results/20260922T140155Z-5ab664002564` |
| light-sweep | 5.310 | 0.333 | 16.544 | 5.751 | `build/magik-results/20260922T153343Z-580107ab4b93` |
| mirror-floor | 4.549 | 0.141 | 16.542 | 4.790 | `build/magik-results/20260922T141304Z-bb3a4ca7a9c3` |
| pixel-dissolve | 2.513 | 1.000 | 16.559 | 4.170 | `build/magik-results/20260922T141810Z-ed8a86efb7af` |
| starfield-comets | 0.219 | 1.331 | 16.550 | 0.314 | `build/magik-results/20260922T142022Z-43e6529ca020` |
| palette-aurora | 4.780 | 1.809 | 16.547 | 5.005 | `build/magik-results/20260922T152157Z-1c6c29439ae2` |
| texture-tunnel | 11.785 | 1.694 | 16.573 | 12.126 | `build/magik-results/20260922T144613Z-0b65f9898c32` |
| raster-waves | 3.230 | 0.968 | 16.558 | 3.436 | `build/magik-results/20260922T151400Z-a253d837819c` |
| wireframe-terrain | 3.820 | 1.563 | 16.552 | 4.066 | `build/magik-results/20260922T151609Z-dd5df7a60aaa` |

## Validation and limitations

- Portable renderer tests cover deterministic reset, clipping, exact dissolve endpoints, fixed seed behavior and retained damage against full frames at 960×540 and 960×600. The existing two-slot damage ledger tests cover alternating slots, initialization and suppressed presentation.
- Latest portable suite: 15 unit tests and two retained-frame tests passed at both geometries. Mini smoke and both motion checks passed on device (`20260922T152047Z-4c2deae9ae72`, `20260922T152113Z-7e4a673f6a74`).
- Separate terrain profiling completed with 905 samples and matching artifact identity (`20260922T152008Z-bb103693141e`). Its four physical repeats are instrumented evidence and do not qualify cadence.
- All listed unprofiled runs reported zero latch drops and rejections, including the failed curved tunnel run.
- Host regression suite: 247 passed. Native service: 47 passed. Mini session: three passed. Tooling measurement/session: six passed. Focused Clippy, Python formatting/lint/type checks and ARM builds passed.
- Normal exit, interruption and controlled failure retained the selected, advancing Mini concept in `build/magik-results/20260922T142918Z-f2d647547cb2/retention-results.json`. Mini now handles termination after completing its frame; the service gives it a bounded graceful-stop interval.
- Depth initially measured 30 FPS; bounded prepared perspective poses fixed that without reducing the five-card default. Dissolve initially measured 30 FPS; prepared tile thresholds and row-span copies fixed that without changing tile size or timing. Initial failed runs are retained in ignored results.
- The USB HDMI capture adapter was unavailable. The user elected to review the live display. Current capture controls produce exact initial, midpoint and loop-boundary evidence, plus a cabinet capture for point-cloud morph. Point-cloud and depth captures were repeated using exact device bookmarks on executable f8fa5c209d63 (`20260922T152403Z-3d836f5c6f86` and `20260922T152507Z-fc685dcb40fb`); their cadence rows retain the earlier measured binary identities. Static captures do not replace the remaining live visual signoff.
- Reduced presets are functional options, not substitutes for default-preset qualification. CRT, desktop preview, combined effects, a controller gallery and full-app integration are outside this milestone.

The original flat tunnel passed cadence but was rejected visually and replaced by a curved 3D tube. Its replacement missed three refreshes in the second window. The subsequent check was interrupted and is not qualification evidence.

See [the iteration guide](visual-concepts.md) for controls and final preset budgets. The current Mini concept stays on HDMI when the host session ends.

## Measured executable identities

- point-cloud-morph: `540a8707326681e174958bf066d15678c103f08e23fb2a0bfed67104fb44dff5`
- depth-parallax: `e61b5202dc54ec494e636e6641e431be5d0806f6d65188069f4d97dc53232d27`
- light-sweep: `23ec177d75503df22710c42cec55cd32eea1d97a4e6761cbefed753f3a56bbff`
- mirror-floor: `a6d78fbc1e43a5aa08534e0bedd62460d1272ea980e71482cf3b446e26495a0d`
- pixel-dissolve: `b1f0033b281e9a16a9dfae73f02a562bedd585274271b8754fa3caa82f5dc126`
- starfield-comets: `b1f0033b281e9a16a9dfae73f02a562bedd585274271b8754fa3caa82f5dc126`
- palette-aurora: `f8fa5c209d63d3b10012a20a41ca084c1cc04f3401cc5776b704d103b6db12b9`
- texture-tunnel: `8276ca840ff967cf79d6fff8df02f82e46379724f9a30191f7d066550b75b00f`
- raster-waves: `f8fa5c209d63d3b10012a20a41ca084c1cc04f3401cc5776b704d103b6db12b9`
- wireframe-terrain: `f8fa5c209d63d3b10012a20a41ca084c1cc04f3401cc5776b704d103b6db12b9`

## Light sweep revision

The user rejected the original stripe and selected a subtle card sheen. The
replacement translates a single soft profile continuously, limits its white
contribution to 6.25%, and preserves labels and the frame. Its two default
measurement windows passed on the revision above; visual smoothness and appearance
await the user review. The earlier stripe result is superseded. See
[research, sources and explicit design choices](light-sweep-research.md).
