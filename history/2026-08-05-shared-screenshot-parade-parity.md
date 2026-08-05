# Shared Screenshot Parade Parity — 2026-08-05

## Scope and revisions

The production screenshot parade raster and scheduler were extracted into
`crates/screenshot-parade`, production retained lifecycle and presentation
ownership, and `framebuffer-scene-lab` gained deterministic macOS and attended
MiSTer routes.

- Clean host revision when the pre-change profile was captured:
  `c2694e28af005374384b17364116a0f5c4cc4584`.
- Installed pre-change runtime revision actually profiled:
  `3f717691fd8c977279b89759ee36dde310902864`.
- Production functional revision delivered and profiled:
  `ca177a588621bd1206d96d745b396e926c73e74a`.
- Final fully assured implementation revision before this evidence note:
  `3e5d117f710ead02f349ca91273e13035d0333a7`.

The installed baseline was coherent and qualified, but it was 17 commits behind
the clean host checkout. The benchmark evidence therefore identifies the
installed baseline as `3f717691`, not `c2694e28`. The post-profile commits only
box the scene-lab enum variant, apply rustfmt, and add Clippy annotations to
internal raster helpers; they do not change production renderer behavior.

## Screenshot packs and deterministic pixels

The attended device workflow resolved the installed Dev pack read-only before
suspending Main:

- Path: `/media/fat/mister-magik-dev/assets/arcade-screenshots-320x320.mmlz4b`
- Size: `24,326,278` bytes
- SHA-256: `387728d3d0cf2aa2f2e5b8d56ecc72f63e8d4afc46ac97476dbd13d3ed360ee3`

The local macOS validation used a deliberately separate fixture pack:

- Path: `build/diagnostics/arcade-preview-current/arcade-screenshots-320x320.mmlz4b`
- Size: `24,529,459` bytes
- SHA-256: `1e7e8f1ac104dfbf3081ddfcb29dddd24318dfce518d972003c77c4ee8dfc234`

A prepared 960x540 capture at seed `0x4d6167694b54696c` and 2,000 ms rendered
76 cards with RGB565 FNV-1a hash `3a1f88e97eb0e0a8`.

The stronger old-versus-shared gate compares every RGB565 pixel directly, rather
than relying on hashes. It passed for the synthetic 220-image archive at seed
`0x4d6167694b54696c`, elapsed times 0, 17, 33, 250, and 1,000 ms, and both:

- 960x540 with the HDMI legacy-half sampling profile.
- 640x480 with the CRT sixteenth-pixel sampling profile.

## Lab and assurance results

- Prepared macOS capture passed. Missing archives and invalid seeds were rejected
  before launch. The streaming macOS preview remained active until its attended
  interrupt.
- The attended MiSTer scene lab used the installed pack without upload, resolved
  1280x720 HDMI geometry, populated from 6 to 84 visible cards at approximately
  60 FPS, reported zero repeated presentations and zero latch drops, and restored
  the launcher after interruption.
- Repository full affected assurance passed all 29 selected checks through a
  non-publishing dry-run pre-push gate. This included screenshot-parade tests,
  MiSTer host logic and pixel parity, scene-lab tests/lint, application modes,
  and the Apple container configuration matrix.
- Rust diagnostics were empty for the shared crate, scene lab, and typed workflow.
  The production integration reported only expected macOS inactive-code notices
  for ARM-only sections.

## Compile-time isolation

Measurement artifact:
`/private/tmp/mister-magik-shared-screenshot-compile-20260805b.json`

- Source revision: `ca177a588621bd1206d96d745b396e926c73e74a`
- Cold build: 28,513 ms
- Five no-op samples: 172, 179, 185, 182, 183 ms; median **182 ms**
- Warmup edit: 3,996 ms
- Five shared edits: 4,177, 3,923, 3,895, 4,187, 3,982 ms; median **3,982 ms**
- The edit samples rebuilt only `mister-magik-screenshot-parade` and
  `mister-magik-framebuffer-scene-lab`; neither Slint nor `apps/mister` compiled.

Both the 500 ms no-op and 4,000 ms warm-edit goals passed.

## Production cadence and diagnostics

Baseline evidence:
`build/agent-benchmarks/screensaver/1785930817/`

Final evidence:
`build/agent-benchmarks/screensaver/1785933407/`

Both runs used the retained `hdmi-1280x720p60` route and measured refresh
`60.0168047053 Hz`. Both qualified.

| Metric | Baseline | Shared | Difference |
|---|---:|---:|---:|
| Unique presentation FPS | 59.999239 | 59.999257 | +0.000017 |
| Repeated refreshes | 0 | 0 | 0 |
| Long completion gaps | 0 | 0 | 0 |
| Presentation failures | 0 | 0 | 0 |
| Latch drops | 0 | 0 | 0 |
| Vsync misses | 0 | 0 | 0 |
| Present errors | 0 | 0 | 0 |
| Render-ahead starvation | 0 | 0 | 0 |
| Superseded frames | 0 | 0 | 0 |
| Reused frames | 0 | 0 | 0 |
| Populated renderer CPU, one core | 50.272% | 49.327% | -0.945 pp |
| Populated process CPU, one core | 83.044% | 80.941% | -2.103 pp |
| Populated render wall mean | 8,430.6 us | 8,272.0 us | -158.6 us |
| Populated render wall P99 | 10,420 us | 10,015 us | -405 us |
| Populated render CPU mean | 8,376.2 us | 8,218.8 us | -157.4 us |
| Populated render CPU P99 | 9,538 us | 9,358 us | -180 us |
| Populated process CPU mean | 13,836.8 us | 13,486.3 us | -350.5 us |
| Populated wall P99 | 16,739 us | 16,803 us | +64 us |
| Populated work P99 | 12,942 us | 13,122 us | +180 us |
| Phase-bank resident bytes | 3,964,912 | 4,041,130 | +76,218 |
| Launcher RSS mean | 78,212.4 KiB | 78,175.3 KiB | -37.1 KiB |
| Launcher RSS maximum | 122,116 KiB | 121,892 KiB | -224 KiB |

Pixel equality and cadence qualification are the parity gates. CPU, timing,
phase-bank residency, and RSS are diagnostic only and do not fail parity.
