<!--
Copyright (C) 2026 Nigel Breslaw
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Consolidated startup-particle qualification

Measured on 2026-08-02 after moving MagiK text and arcade-cabinet development
into the Slint-free shared engine and focused lab. This evidence is descriptive,
not a CI performance gate.

## Compile comparison

Both macOS rows use the same Apple Silicon machine, Rust/Cargo 1.97.1, a fresh
target directory, one cold build, five unchanged invocations, one source-edit
warm-up, and five measured shared-engine edits. The adjacent JSON reports retain
the individual samples and source hashes.

| Path | Cold | No-op median | Particle edit median |
| --- | ---: | ---: | ---: |
| Full Slint application before extraction | 79.267 s | 3.171 s | 3.092 s |
| Focused Slint-free lab after extraction | 14.194 s | 0.289 s | 2.878 s |

The focused ARM lab also completed a repository-owned clean build in 10.46 s.
That is a single build observation, not a five-sample edit median and not
directly comparable with either macOS row.

## MiSTer comparison

The after-change rows are steady-state observations from the attended focused
lab using the standalone framebuffer/latch presenter. Each effect ran for more
than 15 seconds. The reported render-work interval contains simulation,
projection, RGB565 rasterization, and foreground bookkeeping, but ends before
latch presentation waiting. The table records the worst one-second P99/maximum
observed during each run.

| Effect and path | Particles | Physical FPS | Process CPU | Render P99/max | Dropped frames |
| --- | ---: | ---: | ---: | ---: | ---: |
| MagiK before extraction, production launcher | 40,960 | 60.029 | 57.55% | 4.704 / 9.229 ms | 0 |
| MagiK after extraction, focused lab | 40,960 | 60.0 | 50.0–58.5% | 10.774 / 10.774 ms | 0 |
| Cabinet after extraction, focused lab | 12,288 | 60.0 | 49.1–53.3% | 9.465 / 9.465 ms | 0 |

The old renderer P99 is not an apples-to-apples render comparison: asynchronous
simulation and projection were charged to a preparation worker and excluded
from that renderer interval. Total process CPU is the useful directional metric.
The new MagiK run stayed below the 65% CPU and 12 ms render-work-P99 targets; its
valid-reload interval briefly reached 61.2% CPU. Consequently the conditional
projection cache and render-ahead queue were not added.

The MagiK lab reported `armv7-neon-cohort30` simulation and
`armv7-neon-packed-r1` projection. A valid 40,000-particle edit applied while
frames continued, a malformed partial recipe was rejected without replacing
the last-good renderer, and deleting the file restored the 16,384-particle
embedded default after two polls. All three states advanced the status
generation as specified.

The current Dev-launcher workflow now waits for an embedded-ready status before
publishing a recipe and starts its one-second acknowledgement deadline only
after that readiness point. Hardware acknowledgement could not be requalified
against the installed Dev application because its revision
`265bf3e9b666b84f8ca365455118053da38eb074` predates watcher commit `f999684e`.
Installing the current coherent Dev bundle and repeating the attended workflow
remains an operational qualification step; it does not change the implemented
reload contract. Device arming status was clear after the attempt.
