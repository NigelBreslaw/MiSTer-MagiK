<!--
Copyright (C) 2026 Nigel Breslaw
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Startup particle development

The production-quality MagiK text and arcade-cabinet effects share the
Slint-free `crates/particles` engine. They are developed in the focused
`apps/startup-particle-lab` application and consumed by the production launcher
through a thin host adapter. The engine owns deterministic simulation,
validated recipes, target assets, RGB565 drawing commands, and ARM MagiK NEON
kernels. The hosts continue to own preparation threads, framebuffer/latch
lifecycle, and presentation.

The older `apps/framebuffer-lab` showcase remains a separate 36-demo experiment
surface. Its registry, recipe-family maps, and copy of the cabinet experiment
are not part of this production workflow and must not be folded into the shared
engine.

## Recipe contracts

The two checked-in defaults are independent, versioned JSON contracts. Every
object uses Serde `deny_unknown_fields`; parsing produces a DTO first, then a
validated runtime recipe. Invalid schemas, unknown fields, non-finite or unsafe
numbers, bad palette indices, and invalid particle counts fail before renderer
construction.

| Effect | Schema | Checked-in default | Top-level data groups |
| --- | --- | --- | --- |
| MagiK text | `mister-magik-particle-magik-v1` | `crates/particles/assets/recipes/magik-v1.json` | particle count and seed; timing; initial scatter; depth; projection; rotation; static, form, hold, and disperse motion; appearance and four-color palette |
| Arcade cabinet | `mister-magik-particle-cabinet-v1` | `crates/particles/assets/recipes/cabinet-v1.json` | particle count and seed; timing/easing; model scale; source scatter; dispersal; camera formation/orbit/return poses; appearance, eight-color palette, and material/feature emphasis |

The JSON owns effect-facing values. SIMD selection, packed drawing-command ABI,
cohorts, preparation pipeline, PMU measurement, framebuffer setup, and latch
control remain engineering configuration in Rust and C.

Embedded-default validation is fail-closed: a bad checked-in recipe is a startup
error, never an invitation to fall back to hidden Rust constants. Particle and
target order remain stable so a later MagiK-to-cabinet transition can reuse
identity without introducing a transition framework now.

## Live reload contract

The shared watcher polls every 100 ms and reads at most 1 MiB. A distinct file
observation receives a monotonically increasing generation. Parsing and complete
renderer preparation run on the watcher thread; the host takes only the newest
pending generation and swaps it at the next frame boundary. Applying a valid
recipe restarts the effect deterministically at logical time zero.

Rejected and partial saves keep the last-good renderer. Errors written to status
are bounded to 512 bytes. After a previously observed recipe is absent on two
consecutive polls, the host restores its already prepared embedded renderer
once. Repeated missing polls do not create more reset generations.

The host atomically writes `status.json` using schema
`mister-magik-startup-particle-status-v1`. Its `generation`, `recipe`, and
`state` (`embedded`, `applied`, or `rejected`) acknowledge what the host did;
rejected states also contain the bounded error. A file appearing on disk is not
an acknowledgement by itself.

## Runtime boundaries

| Runtime | Mutable recipe behavior |
| --- | --- |
| macOS focused lab | Watches the selected MagiK or cabinet file and writes `status.json` beside it. |
| Attended MiSTer focused lab | Watches the volatile recipe used for the session; accepts MagiK or cabinet. Uses the Dev framebuffer/latch lifecycle and restores the launcher on exit. |
| `MiSTer_MagiKDev` launcher | Watches only `/tmp/mister-magik/startup-particles/magik.json`; acknowledges through `/tmp/mister-magik/startup-particles/status.json`. Only MagiK is accepted. |
| Public `MiSTer_MagiK` launcher | Uses the validated embedded MagiK default. It does not construct a watcher and never opens or polls the Dev recipe path. |

The Dev launcher gate is structural: `DeviceLayout::current()` must be `Dev`
before watcher construction. There is no environment override or persistent
recipe path. Attended device sessions use only volatile `/tmp` state, require a
terminal, and remove their recipe before waiting for an embedded-default
acknowledgement and restoring the launcher.

Use the supported workflow entry points rather than invoking build or transport
details directly:

```text
scripts/agent startup-particles preview RECIPE
scripts/agent device startup-particles RECIPE --runtime lab --attended
scripts/agent device startup-particles RECIPE --runtime dev-launcher --attended
```

The preview and focused-lab modes accept either schema. Dev-launcher mode
rejects cabinet recipes.

For deterministic visual evidence, the focused binary also supports a headless
fixed-time capture. See `apps/startup-particle-lab/README.md` for the direct
binary interface.

## RGB565 authority

Simulation output, palettes, packed commands, golden hashes, cached frames,
device framebuffer writes, and latch slots are RGB565 throughout. There is no
RGB888 particle-engine path and no RGB888 device framebuffer conversion.

Two host-only presentation boundaries expand a finished RGB565 frame because
their external formats require it:

- the macOS window copies RGB565 pixels into the window library's XRGB8888
  surface;
- a headless PPM capture expands RGB565 to three eight-bit channels because the
  PPM file format stores RGB byte triples.

Those conversions are display/export adapters after rendering. Frame hashes are
calculated from the original little-endian RGB565 words, so neither conversion
can change simulation or device output.

## Target and cabinet asset authority

The MagiK text target is the checked-in
`crates/particles/assets/magik-alpha-mask.bin`. Its adjacent provenance file
records the Press Start 2P input, generator, dimensions, and expected hash.

The cabinet model authority is
`crates/particles/assets/cabinet/arcade-cabinet.glb`; the adjacent license notice
records attribution and the CC-BY-NC-4.0 terms. `arcade-cabinet.pcloud` is its
canonical compiled particle target. `PCLOUD1` is little-endian and contains:

- an eight-byte `PCLOUD1\0` magic;
- a `u16` version, `u16` record stride, `u32` point count, and six `i16` bounds;
- fixed eight-byte records containing `i16 x`, `i16 y`, `i16 z`, `u8` material,
  and `u8` feature flags.

The decoder requires version 1, stride 8, the exact expected length and point
count, ordered bounds, in-bounds coordinates, material values 0 through 7, only
the two defined flag bits, and no trailing data.

## Compile-time boundary and evidence

Changing shared simulation, recipes, or the focused lab must not compile Slint
or the rest of `apps/mister`. The focused lab depends directly on
`crates/particles`; only its ARM target adds the narrow framebuffer runtime
dependency needed by the attended latch presenter. Production depends on the
same shared crate, so a validated default and its rendering behavior do not
fork between development and shipping.

The macOS measurements used the same machine and five-sample method. Moving the
edit boundary out of the full application made cold and no-op builds much
faster; the shared-engine edit median is also below three seconds.

| Path | Cold | No-op median | Particle edit median |
| --- | ---: | ---: | ---: |
| Full Slint application before extraction | 79.267 s | 3.171 s | 3.092 s |
| Focused Slint-free lab after extraction | 14.194 s | 0.289 s | 2.878 s |

The focused ARM lab completed a separate build in 10.46 seconds. That is a
single build result, not a five-sample edit median, so it must not be presented
as directly comparable evidence. These timings are dated evidence, not CI
gates. The reports and individual samples live in `history/toolchain-bench/`.

## Device qualification

The consolidated lab preserves physical 60 Hz with no repeated presentations.
The after-change timing includes simulation, projection, RGB565 rasterization,
and their foreground bookkeeping. It ends before latch presentation waiting.

| Effect and path | Particles | Physical FPS | Process CPU | Render P99/max | Repeats |
| --- | ---: | ---: | ---: | ---: | ---: |
| MagiK before extraction, production launcher | 40,960 | 60.029 | 57.55% | 4.704 / 9.229 ms | 0 |
| MagiK after extraction, focused lab | 40,960 | 60.0 | 50.0–58.5% | 10.774 / 10.774 ms | 0 |
| Cabinet after extraction, focused lab | 12,288 | 60.0 | 49.1–53.3% | 9.465 / 9.465 ms | 0 |

The pre-extraction renderer timing excluded asynchronous preparation, so its
P99 must not be compared directly with the lab's synchronous render-work
interval. Process CPU is the useful directional comparison. The focused MagiK
result meets the 65% CPU, 12 ms render-work P99, 16.667 ms maximum, and
zero-repeat targets without a projection cache or render-ahead queue.

Attended lab qualification also covered valid apply, invalid-save rejection
with last-good retention, and the two-poll deletion reset while frames
continued. The implementation work in all 11 consolidation steps is complete.
Dev-launcher hardware acknowledgement still needs one operational rerun after
installing a coherent Dev application that contains the watcher; the installed
revision used on 2026-08-02 predates that code. Detailed scope, timing caveats,
and device observations are recorded in
`history/toolchain-bench/startup-particles-after-20260802.md`.
