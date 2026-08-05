<!--
Copyright (C) 2026 Nigel Breslaw
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Shared framebuffer-scene development

The production-quality MagiK text and arcade-cabinet effects share the
Slint-free `crates/particles` engine. Portable scene contracts and pure launcher
navigation-transition rasterization live in the Slint-free
`crates/framebuffer-scenes` crate. All three are developed in the focused
`apps/framebuffer-scene-lab` application and consumed by production through
thin host adapters.

The shared code owns checked RGB565 targets, deterministic simulation, complete
MagiK lookahead preparation and caching, validated recipes, target assets,
drawing commands, and ARM MagiK NEON kernels. Hosts own Slint snapshots,
navigation/input policy, thread placement, PMU reporting, framebuffer/latch
lifecycle, Main coordination, catalog access, and presentation.

The older `apps/framebuffer-lab` showcase remains a separate 36-demo experiment
surface. Its registry, recipe-family maps, and copy of the cabinet experiment
are not part of this production workflow and must not be folded into the shared
engine.

## First-run launcher intro

The production launcher uses the embedded `intro-v1` scene only for
`ColdNoCatalog`: no return capsule, no Catalog V3 registry, and no valid
retained Arcade bootstrap projection. A valid registry or retained projection
is a warm start and goes directly to the interactive launcher. The retained
projection is source-stamped, bounded, and checksummed, so an interrupted first
run repeats the intro unless it reached the first-visible publication barrier.

The intro owns the production direct hidden-slot latch for 20 seconds of
storyboard time. Storyboard time advances by the resolved physical refresh
period only after the posted sequence becomes the active scanout sequence at a
confirmed physical vblank, and is clamped to exactly 20 seconds. Physical frame
numbering remains independent for projection cohorts and flicker. Acceptance or
pending status is insufficient. A missing grant, failed post, or incomplete
latch confirmation retries the same storyboard timestamp. The expected post
count is therefore `ceil(20 seconds / refresh_period)`, plus any cabinet-wait
frames. The catalog builder starts
with the intro and reserves CPU1 for rendering: its coordinator, Arcade
bootstrap, walker, audit, projection, snapshot, and persistence work all use
the continuous CPU0 background policy. While particles own the display, normal
Slint timer dispatch, bridge synchronization, and launcher rendering are
suppressed. Catalog/UI changes are coalesced in Rust state rather than copied
into an invisible Slint tree. Transient catalog scan milestones do not clone
navigation shells or rebuild launcher taxonomy on CPU1; the authoritative
published projection supplies that state.

HDMI retains the original 102,400 initial and 40,960 steady particles. All four
resolved CRT routes (`crt-240p60`, `crt-288p50`, `crt-480p60`, and
`crt-576p50`) use deterministic half-density tracks with 51,200 initial and
20,480 steady particles. Thinning occurs independently within each MiSTer/MagiK
letter track, preserving paired morph identity, group alignment, and cabinet
ordering. CRT projection fits the complete 16:9 scene inside the 4:3 raster:
X uses scale `2/3`, Y uses `framebuffer_height/720`, and the resulting frame is
centred with equal top and bottom margins.

Cadence and latch health are independent. `latch_drop_count=0` proves only that
the FPGA latch did not reject or supersede a post. Skipless animation requires
completion timestamps and flip deltas to prove that every physical refresh
received one new confirmed frame. Runtime and benchmark evidence therefore
reports `skipped_refreshes` separately from latch protocol drops and fails if
either axis is nonzero.

The final handoff is derived only from the production launcher's live frame:

1. The existing lifecycle `launcher_reveal_ready` transition is the sole
   launcher-ready event. The normal reveal cycle commits Arcade navigation,
   performs the established full bridge synchronization that populates the
   launcher and its tile models, and clears the scan overlay. The host then
   performs one off-screen Slint render into the persistent RGB565 composition
   cache. It does not switch to `NewBuffer` and synchronously repaint unchanged
   cache regions while the intro owns scanout.
   It immediately
   snapshots that exact cache, then a low-priority CPU0 worker derives all
   40,960 HDMI or 20,480 CRT launcher particle positions and colors without
   blocking CPU1. No
   design-time launcher image or point cloud is embedded.
2. If no usable launcher frame exists at 16 seconds, logical storyboard time
   pauses and the fully formed cabinet keeps spinning at the resolved physical
   refresh cadence. Each four-second
   source orbit flowed at 0.4 turns per four seconds, so the wait continues at
   exactly that velocity: one seamless full turn every ten seconds. Once the
   real launcher frame is ready, the normal morph begins.
3. From 16 to 18 seconds the cabinet particles morph into that live launcher
   formation. At 18 seconds both hidden slots settle on the static target;
   repeated hold frames perform no pixel writes.
4. From 19 to 20 seconds the particle scene uses incremental Bayer buckets to
   crossfade into the same retained live frame without allocation or a
   full-frame rewrite.
5. The frame at exactly 20 logical seconds is pixel-identical to the cached launcher.
   The host restores the same pixels to the interactive cache, returns the
   hidden mappings, leaves external-direct mode, and only then enables startup
   input.

The intro renders directly at the native CRT framebuffer size. On 240p, the
live 640×480 launcher cache is converted to the 640×240 morph target with the
same centred nearest-row transform used by ordinary presentation; the original
640×480 cache remains retained and is restored at handoff. The other CRT routes
use their native 640×288, 640×480, or 640×576 cache geometry directly. Any
route, preparation, grant, latch, or transform failure fails open to the
ordinary launcher. A delayed catalog never
fails the intro or reveals a loading UI; it extends only the spinning-cabinet
wait. The recipe renderer itself remains geometry-aware so focused-lab captures
can exercise non-960x540 sizes.

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
| Attended MiSTer focused lab | Watches the volatile recipe used for the session; accepts MagiK or cabinet. Uses the Dev framebuffer/latch lifecycle, scales its 960x540 RGB565 source to Main's confirmed fixed HDMI output rectangle in the FPGA, and restores the launcher on exit. CRT routing remains production-owned. |
| Navigation fixture lab | Uses an immutable generated `home-arcade`, `home-consoles`, or `consoles-system` RGB565 fixture and cycles forward/reverse. It opens no recipe, Slint snapshot, or catalog. |
| Screenshot screensaver lab | Shares the production screenshot parade renderer. macOS preview/capture reads an explicit archive; the attended MiSTer workflow validates and reads the installed Dev Arcade screenshot pack without uploading it. Production retains pack discovery, lifecycle, render-ahead, and latch presentation ownership. |
| `MiSTer_MagiKDev` launcher | Watches only `/tmp/mister-magik/startup-particles/magik.json`; acknowledges through `/tmp/mister-magik/startup-particles/status.json`. Only MagiK is accepted. |
| Public `MiSTer_MagiK` launcher | Uses validated embedded particle defaults, including the cold-catalog intro. It does not construct a watcher and never opens or polls the Dev recipe path. |

The Dev launcher gate is structural: `DeviceLayout::current()` must be `Dev`
before watcher construction. There is no environment override or persistent
recipe path. Attended device sessions use only volatile `/tmp` state, require a
terminal, and remove their recipe before waiting for an embedded-default
acknowledgement and restoring the launcher.

Use the supported workflow entry points rather than invoking build or transport
details directly:

```text
scripts/agent startup-particles preview RECIPE
scripts/agent scene-lab preview --scene magik --recipe RECIPE
scripts/agent scene-lab preview --scene cabinet --recipe RECIPE
scripts/agent scene-lab preview --scene navigation-transition --fixture home-arcade
scripts/agent scene-lab preview --scene card-flip
scripts/agent scene-lab preview --scene screenshot-screensaver --archive FILE [--seed SEED]
scripts/agent scene-lab capture --scene screenshot-screensaver --archive FILE [--seed SEED] --time-ms N --output FILE
scripts/agent device scene-lab --scene magik --recipe RECIPE --attended
scripts/agent device scene-lab --scene navigation-transition --fixture home-arcade --attended
scripts/agent device scene-lab --scene card-flip --attended
scripts/agent device scene-lab --scene screenshot-screensaver [--seed SEED] --attended
scripts/agent device startup-particles RECIPE --runtime lab --attended
scripts/agent device startup-particles RECIPE --runtime dev-launcher --attended
```

`scene-lab` is canonical. The older `startup-particles preview` command and
attended `--runtime lab` path remain thin typed compatibility aliases; the old
particle-only lab app/binary name is compatibility-only. Particle preview and
focused-lab modes accept either schema, while Dev-launcher mode rejects cabinet
recipes. Navigation accepts `--fixture`, never `--recipe`.

`card-flip` is a procedural, self-contained scene. It has no recipe, fixture,
binary face assets, or generator. macOS supplies the readable reference path;
MiSTer owns a separate fixed-point renderer that draws directly at the shared
resolved display geometry. The shared cached hidden-latch presenter owns slot
history and dirty-region transfer under the same timing, palette, border, and
corner contract used by MagiK. Each face repeats the same door-hinge trajectory
so continuous flips keep one perceived rotation direction.

For deterministic visual evidence, the focused binary also supports a headless
fixed-time capture. See `apps/framebuffer-scene-lab/README.md` for the direct
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

`arcade-cabinet.pcolor` is the aligned `PCOLOR1` colour sidecar generated from
the GLB base-colour texture and the exact progressive `PCLOUD1` point order. Its
16-byte little-endian header contains the eight-byte `PCOLOR1\0` magic, `u16`
version, `u16` stride, and `u32` point count. Each four-byte record stores a
faithful RGB565 texture sample followed by a visibility-lifted RGB565 sample.
The runtime validates the exact count, stride, and length once, then performs
only sequential packed-colour reads while rasterizing.

## Compile-time boundary and evidence

Changing shared scene rendering, recipes, or the focused lab must not compile
Slint or the rest of `apps/mister`. The focused lab depends directly on
`crates/framebuffer-scenes`, `crates/particles`, and
`crates/screenshot-parade`; only its ARM target adds the
narrow framebuffer runtime dependency needed by the attended latch presenter.
The screenshot crate reads the resident catalog archive but contains no Slint,
Main, latch, framebuffer ioctl, `/dev/fb0`, device-layout, or presentation
dependencies.

The macOS measurements used the same machine and five-sample method. Moving the
edit boundary out of the full application made cold and no-op builds much
faster; shared-renderer edits remain around three seconds and lab-host edits
stay below one second.

| Scene-lab edit boundary | Cold | No-op median | Five-sample edit median |
| --- | ---: | ---: | ---: |
| Shared MagiK renderer | 9.701 s | 0.184 s | 2.894 s |
| Shared navigation rasterizer | 8.924 s | 0.181 s | 3.143 s |
| Lab host | 8.867 s | 0.184 s | 0.776 s |
| Shared screenshot parade | qualification recorded separately | target ≤ 0.500 s | target ≤ 4.000 s |

Use the repository-owned measurement inputs explicitly:

```text
scripts/agent compile-time measure framebuffer-scene-lab-macos --edit shared-magik --target-dir NEW_ABSOLUTE_PATH --output NEW_JSON_PATH
scripts/agent compile-time measure framebuffer-scene-lab-macos --edit shared-navigation --target-dir NEW_ABSOLUTE_PATH --output NEW_JSON_PATH
scripts/agent compile-time measure framebuffer-scene-lab-macos --edit shared-screenshot-parade --target-dir NEW_ABSOLUTE_PATH --output NEW_JSON_PATH
scripts/agent compile-time measure framebuffer-scene-lab-macos --edit lab-host --target-dir NEW_ABSOLUTE_PATH --output NEW_JSON_PATH
```

The focused ARM lab completed a separate build in 10.46 seconds. That is a
single build result, not a five-sample edit median, so it must not be presented
as directly comparable evidence. These timings are dated evidence, not CI
gates. The reports, source hashes, and individual samples live in
`history/toolchain-bench/framebuffer-scene-lab-*-20260802.json`.

## Device evidence and pending qualification

Earlier attended particle-lab evidence preserved physical 60 Hz with no
repeated presentations. Its after-change timing includes simulation,
projection, RGB565 rasterization, and foreground bookkeeping, and ends before
latch presentation waiting. It predates the final shared-renderer migration and
must not be represented as final device qualification for this extraction.

| Effect and path | Particles | Physical FPS | Process CPU | Render P99/max | Repeats |
| --- | ---: | ---: | ---: | ---: | ---: |
| MagiK before extraction, production launcher | 40,960 | 60.029 | 57.55% | 4.704 / 9.229 ms | 0 |
| MagiK after extraction, focused lab | 40,960 | 60.0 | 50.0–58.5% | 10.774 / 10.774 ms | 0 |
| Cabinet after extraction, focused lab | 12,288 | 60.0 | 49.1–53.3% | 9.465 / 9.465 ms | 0 |
| Navigation fixture after extraction | n/a | Pending | Pending | Pending | Pending |

The pre-extraction renderer timing excluded asynchronous preparation, so its
P99 must not be compared directly with the lab's synchronous render-work
interval. Process CPU is the useful directional comparison. The earlier
focused MagiK result met the 65% CPU, 12 ms render-work P99, 16.667 ms maximum,
and zero-repeat targets. The final shared MagiK scene does include the qualified
two-cohort projection cache, lookahead preparation, ordered commands, and
per-buffer dirty history moved from production.

The earlier attended lab run also covered valid apply, invalid-save rejection
with last-good retention, and the two-poll deletion reset while frames
continued. Final MagiK, cabinet, navigation-fixture, and Dev-launcher device
qualification requires a clean coherent Dev delivery. It was not run during
this migration because device delivery requires separate user authorization.
Detailed prior scope, timing caveats, and device observations are recorded in
`history/toolchain-bench/startup-particles-after-20260802.md`; the current
qualification status is recorded in
`history/2026-08-02-shared-framebuffer-scenes-qualification.md`.

## Next screensaver migration

The existing 20-mode launcher screensaver remains application-owned in this
phase. Its next safe migration uses the same seam: the production host loads
archives/catalog entries and decodes assets, then passes owned RGB565 pixels and
typed scene inputs into a portable scene. Archive traversal, catalog state,
Slint state, route selection, and presentation do not move into the shared
crate. No generic scene registry, scene graph, or cross-scene JSON schema is
introduced.
