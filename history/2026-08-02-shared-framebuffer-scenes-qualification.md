<!--
Copyright (C) 2026 Nigel Breslaw
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Shared framebuffer-scene qualification

This dated record describes the extraction from clean starting revision
`5b002fac`. It is evidence, not current policy; current ownership and workflows
are documented in `docs/architecture.md` and `docs/startup-particles.md`.

## RGB565 correctness

The production and focused-lab renderers retained the pre-extraction hashes.
Hashes are FNV-1a over little-endian RGB565 words.

| Scene or production edge | Direction/time | Hash |
|---|---|---:|
| MagiK embedded recipe | 5,000 ms | `aa213d52dc7eeeef` |
| cabinet embedded recipe | 5,000 ms | `01938d06cd4378ff` |
| home-consoles production transition | forward frame 18 | `4a83d4978f99ef38` |
| home-consoles production transition | reverse frame 18 | `9c86b69cfdf1f735` |
| home-arcade production transition | forward frame 18 | `91027672e7f5a9b1` |
| home-arcade production transition | reverse frame 18 | `c45c776502db2adc` |
| consoles-system production transition | forward frame 18 | `62c16f8b5b5cfeaf` |
| consoles-system production transition | reverse frame 18 | `f6c8a5390b79115b` |

Generated navigation lab fixtures have separate deterministic forward/reverse
hashes in the focused-lab tests. They intentionally do not equal production
captures because they use generated cards/backgrounds rather than Slint
snapshots or catalog content.

## Compile boundary

Each macOS row used a new target directory, one cold build, five unchanged
builds, one edit warm-up, and five measured edits on the same Apple Silicon
host. The repository workflow restored each touched source byte-for-byte and
recorded matching before/after SHA-256 values.

| Edit boundary | Cold | No-op median | Edit median | Report |
|---|---:|---:|---:|---|
| shared MagiK renderer | 9.701 s | 0.184 s | 2.894 s | `history/toolchain-bench/framebuffer-scene-lab-shared-magik-20260802.json` |
| shared navigation rasterizer | 8.924 s | 0.181 s | 3.143 s | `history/toolchain-bench/framebuffer-scene-lab-shared-navigation-20260802.json` |
| lab host | 8.867 s | 0.184 s | 0.776 s | `history/toolchain-bench/framebuffer-scene-lab-lab-host-20260802.json` |

The focused build graph contains `crates/framebuffer-scenes`,
`crates/particles`, and `apps/framebuffer-scene-lab`; it does not contain Slint
or `apps/mister`. The lab's MiSTer runtime dependency remains ARM-target-only
and owns presentation rather than scene rendering.

## Device comparison and authorization status

The prior attended particle evidence below is retained for directional context.
It predates the final shared-renderer extraction and is not relabeled as current
qualification.

| Effect/path | Source/destination | Physical FPS | CPU | P99/max | Dropped frames | Status |
|---|---|---:|---:|---:|---:|---|
| MagiK pre-split production | 960x540 / 1920x1080 | 60.029 | 57.55% | 4.704 / 9.229 ms | 0 | historical baseline |
| MagiK earlier focused lab | 960x540 / 1920x1080 | 60.0 | 50.0–58.5% | 10.774 / 10.774 ms | 0 | historical directional evidence |
| cabinet earlier focused lab | 960x540 / 1920x1080 | 60.0 | 49.1–53.3% | 9.465 / 9.465 ms | 0 | historical directional evidence |
| final shared MagiK | 960x540 / 1920x1080 | pending | pending | pending | pending | clean Dev delivery required |
| final shared cabinet | 960x540 / 1920x1080 | pending | pending | pending | pending | clean Dev delivery required |
| navigation fixture cycle | 960x540 / 1920x1080 | pending | pending | pending | pending | clean Dev delivery required |
| Dev-launcher apply/reset acknowledgement | n/a | n/a | n/a | target <=1 s | n/a | clean Dev delivery required |

No delivery or post-delivery device qualification was run in this migration.
The repository requires separate user authorization for the clean development
delivery, so these pending cells remain explicit rather than inferred from host
builds or older device sessions. No reset-fault or reboot workflow was used.
