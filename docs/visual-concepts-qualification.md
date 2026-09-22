# Mini visual concepts — device qualification

Status: live HDMI review is complete. Five effects are retained: light sweep, pixel dissolve, starfield, the corrected curved tunnel and raster waves. Each has user acceptance and passing recorded device windows at the executable identities below. The other five concepts were removed after review.

These are the latest completed measurements for each concept, across the explicitly identified development binaries below. A prior binary pass does not qualify a changed renderer.

Observed mode: HDMI 1920×1080p60; resolved RGB565 rendering: 960×540. No display-mode changes. Each row uses the default preset, two unprofiled 30-second device-clock windows and two-second warmups. Captures and playback checks run afterward. Streaming publication is disabled during measurement.

| Concept | FPS W1 / W2 | Samples W1 / W2 | Repeats W1 / W2 | CPU % W1 / W2 | Peak RSS MiB | Device gate |
|---|---:|---:|---:|---:|---:|---|
| light-sweep | 60.000 / 60.000 | 1800 / 1800 | 0 / 0 | 100.18 / 100.16 | 34.3 | Pass |
| pixel-dissolve | 59.998 / 59.998 | 1800 / 1800 | 0 / 0 | 100.15 / 100.23 | 65.3 | Pass |
| starfield | 59.998 / 60.000 | 1800 / 1800 | 0 / 0 | 100.13 / 100.13 | 34.2 | Pass |
| texture-tunnel | 59.998 / 59.998 | 1800 / 1800 | 0 / 0 | 100.24 / 100.18 | 68.4 | Pass |
| raster-waves | 60.000 / 60.000 | 1800 / 1800 | 0 / 0 | 100.17 / 100.23 | 34.0 | Pass |

Required gates: zero protocol-v5 physical repeats, zero latch drops/rejections, valid ownership, one advanced timeline frame per presentation, CPU below 150% in each window, process peak RSS at most 128 MiB. Actual refresh comes from owned-vblank deltas rather than assuming exactly 60 Hz.

## Timings and evidence

Times below are the larger average of the two windows, in milliseconds. Frame-to-present includes rendering, transfer and settled latch wait; p99 is the larger render p99. Raw logs retain each window and maxima.

| Concept | Render avg | Transfer avg | Frame-to-present avg | Render p99 | Ignored evidence directory |
|---|---:|---:|---:|---:|---|
| light-sweep | 5.274 | 0.331 | 16.551 | 5.811 | `build/magik-results/20260922T163217Z-575a01f0d75d` |
| pixel-dissolve | 2.513 | 1.000 | 16.559 | 4.170 | `build/magik-results/20260922T141810Z-ed8a86efb7af` |
| starfield | 1.311 | 1.382 | 16.550 | 1.460 | `build/magik-results/20260922T155324Z-8b0a2f56e128` |
| texture-tunnel | 3.924 | 1.651 | 16.561 | 4.116 | `build/magik-results/20260922T161109Z-bdb96d29b2ba` |
| raster-waves | 3.230 | 0.968 | 16.558 | 3.436 | `build/magik-results/20260922T151400Z-a253d837819c` |

## Validation and limitations

- The five retained effects were accepted by the user on HDMI on 22 September 2026. The tunnel was accepted after correcting its motion glitch.
- Cleanup validation: 17 portable unit tests, two retained-frame/reset tests, three Mini tests, six tooling tests, 247 host tests, focused Clippy and repository Python quality checks passed. All 120 sampled before/after RGB565 hashes matched across both presets and both geometries. The cleanup binary passed both light-sweep device windows, captures and playback checks.
- Portable tests cover deterministic reset, clipping, dissolve endpoints, filtered stars, the tunnel's tight bend and retained damage at 960×540 and 960×600. Existing hidden-slot ledger tests cover alternating slots and initialization.
- Earlier device smoke and motion checks passed (`20260922T152047Z-4c2deae9ae72`, `20260922T152113Z-7e4a673f6a74`). Normal exit, interruption and controlled failure retained the advancing Mini concept (`20260922T142918Z-f2d647547cb2/retention-results.json`). The later tunnel cache requires the documented 60-second concept startup allowance.
- The user reviewed animation directly on HDMI; captures came from FPGA-latched scanout slots. Profiling is separate from cadence qualification. Reduced presets do not substitute for default qualification.
- Results above belong to the stated binaries, not a repeated full-suite qualification of every later cleanup build. Before/after portable RGB565 comparisons guard accepted appearance during deletion cleanup.

See [the iteration guide](visual-concepts.md) for controls, preparation costs and scope.

## Measured executable identities

- light-sweep: `4b54accf48f115c24a4e7eb134624970f7898bb8555e5246309b9b64ea9ea0f7`
- pixel-dissolve: `b1f0033b281e9a16a9dfae73f02a562bedd585274271b8754fa3caa82f5dc126`
- starfield: `f34d6e0c445b47dba164dc7e0af40cfc67b642f9aedb98f76725a7631d46b1f9`
- texture-tunnel: `4360d72127c87ef39846a68d6ef9ab39661caa16a7b61778e1f1d165df0e5b79`
- raster-waves: `f8fa5c209d63d3b10012a20a41ca084c1cc04f3401cc5776b704d103b6db12b9`

## Review decisions

Kept: light sheen, pixel dissolve, starfield without comets, corrected curved tunnel,
and raster waves. Removed: mirror floor, palette aurora, wireframe terrain,
point-cloud morph, and depth/parallax. Removed effects have no renderer or selection
controls. The production particle crate and shared card reflections remain owned
by their existing consumers.

The deletion review removes unused fixture helpers, unnecessary game-list buffers
from sheen/waves, a no-op effect reset callback, retained tunnel source-texture
storage and a misleading host cleanup log. Raw evidence and captures remain ignored.
