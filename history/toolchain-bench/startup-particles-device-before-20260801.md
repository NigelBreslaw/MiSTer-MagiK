<!--
Copyright (C) 2026 Nigel Breslaw
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Startup-particle device baseline before extraction

The last qualified fixed MagiK particle trial before the shared-engine
extraction ran on 2026-08-01 from revision
`d79e3169f657edd34e4f62dd36970c99f0f742d1`. Git records that revision as an
ancestor of extraction commit `e4aa4310`.

| Metric | Result |
| --- | ---: |
| Geometry | 960x540 RGB565 |
| Particles | 40,960 |
| Duration | 15 s |
| Unique physical FPS | 60.029414 |
| Process CPU | 57.55% of one core |
| Preparation CPU | 26.01% of one core |
| Renderer CPU | 21.11% of one core |
| Renderer wall P99 | 4.704 ms |
| Renderer wall maximum | 9.229 ms |
| Repeated refreshes | 0 |
| Qualified | yes |

The renderer-wall boundary is not an inclusive frame cost. Simulation and
projection ran asynchronously on the preparation worker, while renderer wall
covered the foreground preparation wait, clear, and raster work. Process CPU
is the appropriate directional comparison with the focused lab because it
includes both threads.

The ignored source artifact is
`build/agent-benchmarks/particle-demo-40k/1785608306/summary.json` with SHA-256
`bb410a4cb4bcc588c63331f7f1fc5878f8c788ef9d05e45195591bc110e46383`.
It identifies the packed projection backend as `armv7-neon-packed-r1`, the
simulation backend as `armv7-neon-cohort30`, and the installed application hash
as `b0717e50317ca2e108841b109ee4455bb37965262f98a82dd27b5694d26f7b69`.
