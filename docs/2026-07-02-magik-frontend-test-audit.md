# MiSTer MagiK Frontend Test Audit

Date: 2026-07-02

Scope: `magik-gui/` frontend and catalog tests, with emphasis on end-user
outcomes: fast boot, correct HDMI/framebuffer ownership, smooth controller
navigation, reliable catalog discovery, quick previews, and safe game launch.

This audit used four parallel codebase reviewers over:

- startup, display, framebuffer, controller input, and boot lifecycle,
- catalog/library/game discovery,
- preview/media/cache handling,
- launcher navigation, launch handoff, scheduler, composition, and effects.

I also did a local synthesis pass over the reported hotspots.

## Commands Run

```bash
cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features -- --list
cargo test --manifest-path magik-gui/catalog/Cargo.toml -- --list
cargo test --manifest-path magik-gui/catalog/Cargo.toml
cargo test --manifest-path magik-gui/Cargo.toml --lib --no-default-features
cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features
```

Reported by subagents:

- `magik-gui/catalog`: 246 passed.
- `magik-gui` lib no default features: 244 passed.
- `magik-gui` with `ui`: 354 passed.

The `--list` pass showed 271 lib-target tests and 354 binary-target tests for
`mister-magik-fb`, plus 246 catalog tests. Some tests are compiled through both
library and binary targets because the binary re-declares modules that the
library also exposes, so raw test counts overstate unique behavioral coverage.

## Executive Summary

The tests are much better than a typical AI-written suite. They include many
specific regression tests for real product behavior: catalog media pruning,
launch handoff safety, warm/cold startup reveal, return-to-launcher restore,
RGB565 copy contracts, search/filter navigation, preview sidecar fallback, and
download/publish safety.

The main quality gap is not "no tests." It is missing journey-level coverage
across seams. The suite is strong at module-level invariants, but weaker where
user-visible failures cross module boundaries: boot route setup, launch failure
recovery, media download making a missing preview immediately visible, and the
runtime loop suppressing preview/list work while launching.

Highest-priority risks:

1. `LauncherLifecycle` accepts stale launch worker events from non-launch
   states. A late `LaunchFailed`, `LaunchSucceeded`, or `LaunchTimedOut` can
   transition the lifecycle even if the launcher is no longer launching
   (`magik-gui/src/ui_runner/launcher_lifecycle.rs:602`). Add guard tests and
   likely fix the state machine.
2. Preview/media publishing invalidates archive metadata, but the higher-level
   failed-preview cache can keep a missing preview blank for up to five minutes
   after a pack becomes available (`magik-gui/src/preview_state.rs:141`,
   `magik-gui/src/ui_runner/media_worker.rs:1099`).
3. `ui_boot` has only a route-plan style test. The critical startup sequence
   that prevents black HDMI or wrong framebuffer ownership is not covered as a
   mockable integration flow (`magik-gui/src/ui_runner/ui_boot.rs:15`).
4. Catalog tests are strong in pieces, but there is no single production-shaped
   first-scan fixture that combines real launchables, installed cores, helper
   media clutter, archives, MGLs, and save/load.
5. `launcher_scheduler` has very thin tests for a module that owns catalog,
   media, and launch interleaving (`magik-gui/src/ui_runner/launcher_scheduler.rs:248`).

## What Is Already Strong

### Launcher Navigation And Return State

`magik-gui/src/launcher.rs` is a bright spot. It tests the way a controller user
actually moves through the product:

- alphabet and filter flows around `launcher.rs:3068`,
- search result launch around `launcher.rs:3316`,
- launch return state capture/restore around `launcher.rs:3860`,
- quick taps, holds, turbo movement, edge clamps, and retargeting around
  `launcher.rs:4227`.

These are high-value tests because they describe the user journey instead of
only asserting helper outputs.

### Startup Lifecycle

`magik-gui/src/ui_runner/launcher_lifecycle.rs:791` covers important startup
states:

- cold boot splash timing,
- warm boot holding black until a real frame is ready,
- return-from-game waiting for restored context and preview,
- input gating before reveal,
- loading frame before handoff,
- recovery frame before returning idle.

This is exactly the kind of behavior-focused state-machine testing the codebase
should continue to add.

### Catalog Discovery

The catalog suite is broad and product-aware:

- media and helper pruning around `magik-gui/catalog/src/catalog_scan.rs:934`,
- NeoGeo zip virtual launches around `catalog_scan.rs:806`,
- ColecoVision loose/zip persistence around `catalog_scan.rs:1110`,
- installed generic core gating around
  `magik-gui/catalog/src/launch_profiles.rs:889`,
- AO486 attached-media behavior around `launch_profiles.rs:952`,
- mixed RAM/SQLite parity around `magik-gui/catalog/src/library_db.rs:1419`,
- MAME/HBMAME and console identity coverage in
  `magik-gui/catalog/src/software_identity.rs`.

These tests directly protect "my games show up, helper files do not."

### Preview And Media Runtime

Preview/media tests already cover several painful embedded-device failures:

- indexed `pread` before full archive memory load
  (`magik-gui/catalog/src/preview_worker.rs:3148`),
- invalid sidecars falling back to archive memory
  (`preview_worker.rs:3183`),
- oversized metadata rejected before allocation
  (`preview_worker.rs:3437`),
- missing archive assets not falling back to original screenshot decode
  (`preview_worker.rs:3611`),
- staged pack publish validates before replacing the current pack
  (`magik-gui/src/ui_runner/media_worker.rs:2077`),
- index-only repairs do not show noisy UI progress
  (`media_worker.rs:2129`),
- media work defers behind the interaction gate
  (`magik-gui/src/ui_runner/screenshot_media_update_session.rs:356`).

### Framebuffer And Pacing Contracts

Low-level RGB565/framebuffer tests are valuable:

- display geometry and runtime-vs-INI fallback in
  `magik-gui/src/ui_display.rs:414`,
- padded RGB565 stride and clipped presents in
  `magik-gui/src/framebuffer/mapped.rs:1148`,
- 50/60 Hz pacing and fallback behavior in
  `magik-gui/src/framebuffer/vsync.rs:375`,
- direct-layer composition invariants in
  `magik-gui/src/ui_runner/launcher_composition.rs:241`.

These are correctly aimed at the MiSTer-specific failure modes, not desktop GUI
assumptions.

## Low-Quality Or Brittle Tests

### Implementation-Detail Assertions

`magik-gui/src/ui_runner/launcher_lifecycle.rs:966` asserts
`effects.capacity() == 8`. That locks an internal vector capacity rather than a
product outcome. Replace it with assertions on lifecycle state and emitted
effect names.

`magik-gui/src/ui_runner/launcher_catalog_session.rs:637` and
`launcher_catalog_session.rs:660` collapse effect streams into ordered string
vectors. That is useful in moderation, but brittle when harmless internal
ordering changes. Prefer assertions over final user-visible worker intent,
dialog state, and readiness state.

### Giant Encoded Command Assertions

`magik-gui/src/launcher.rs:4593` asserts one large encoded launch command
string. The scenario is important, but a parser/assertion helper would better
separate protocol regressions from formatting churn.

### Smoke Tests That Do Not Assert Quality

Effect tests such as `magik-gui/src/transition_effects.rs:1854`,
`magik-gui/src/text_effects.rs:3190`, and
`magik-gui/src/sprite_effects.rs:1991` mostly prove "deterministic and moving."
That catches blank frames, but it would still pass ugly, over-budget, or
visually stale effects. Keep these smoke tests, but add small pixel/quality and
budget checks for production paths.

### Optional Fixture Tests That Often Do Nothing

`magik-gui/catalog/src/arcade_catalog.rs:1692` silently returns if a private TSV
fixture is absent. That is fine as optional validation, but it should not count
as CI confidence. Consider marking it ignored or logging a clear skipped-fixture
message.

## Missing Tests And Recommended Improvements

### P0: Guard Stale Launch Events

Problem: `LauncherLifecycleInput::LaunchFailed`, `LaunchSucceeded`, and
`LaunchTimedOut` transition regardless of current lifecycle state
(`magik-gui/src/ui_runner/launcher_lifecycle.rs:602`). A late worker message
could show recovery UI over an idle launcher or move to handoff after recovery.

Add tests:

- stale `LaunchFailed` is ignored in `Idle`, `CatalogReady`, and `Recovered`,
- stale `LaunchSucceeded` is ignored outside `Launching`/`Handoff`,
- stale `LaunchTimedOut` is ignored outside `Launching`/`Handoff`,
- current valid launch failure still presents recovery and returns input only
  after the recovery frame is presented.

Likely implementation: guard these inputs with `matches!` over the launch
states and emit no effects otherwise.

### P0: Add Loop-Level Launch Failure Journey

Current tests cover launch pieces, but not the full loop behavior. Add a
test harness around the logic near:

- `magik-gui/src/ui_runner/launcher_loop.rs:1175`,
- `launcher_loop.rs:1603`,
- `launcher_loop.rs:1950`.

Scenario:

1. User presses A on an Arcade game.
2. Loading frame is presented before worker handoff starts.
3. Worker fails.
4. Recovery frame is presented.
5. Launcher returns idle, accepts input again, and preview/list rendering
   resumes.

This protects the living-room failure mode: "a game failed to launch, but the
launcher came back cleanly."

### P0: Boot/Framebuffer Startup Integration Harness

`magik-gui/src/ui_runner/ui_boot.rs:252` only checks route dimensions by
reconstructing a route. It does not exercise the startup sequence that protects
HDMI:

- detect runtime geometry,
- write temporary RGB565 mode,
- clear black,
- detect display config,
- enable FPGA route,
- optionally settle black frames.

Extract a small trait/fake boundary for framebuffer and FPGA operations, then
add order/failure tests. This should not require `/dev/fb0` or `/dev/mem`.

### P1: Preview Becomes Available After Media Download

Problem: `PreviewImageCache` stores failed paths for five minutes
(`magik-gui/src/preview_state.rs:141`). Media pack publish invalidates archive
metadata (`magik-gui/src/ui_runner/media_worker.rs:1099`), but does not
obviously clear the higher-level failed-preview cache. A user may download a
pack and still see blank previews in the same session.

Add tests:

- missing preview caches a failure,
- media pack publish for that system occurs,
- same preview key can be requested and displayed immediately,
- stale failure entries do not suppress newly available previews.

Likely implementation: add a media-published event path that clears failed
preview entries for the affected archive/system.

### P1: Production-Shaped Catalog First-Scan Fixture

The catalog tests cover many pieces, but not one full realistic SD-card layout.
Add one end-to-end fixture that combines:

- `_Arcade` MRA/MGL launchers,
- `games/<core>` loose payloads,
- installed generic cores,
- `_DOS Games`,
- helper/media decoys,
- zip entries,
- duplicate MGL-covered payloads,
- save/load through SQLite.

Assert:

- visible systems and counts are correct,
- launch refs and structured plans are correct,
- helper files never become games,
- RAM catalog and SQLite load agree.

### P1: Catalog Media-Pruning Parity

`catalog_scan.rs` ignores several media/helper names, but tests do not cover all
of them. Add mixed-case fixtures for:

- `images`,
- `manuals`,
- `screenshots`,
- `screenshot-magik`,
- `boxart`.

Also add `core_audit` parity for singular `screenshot`,
`screenshot-magik`, and `boxart` around
`magik-gui/catalog/src/core_audit.rs:313`, so media-only folders do not become
noisy uncataloged audit rows.

### P1: Scheduler Interleaving

`magik-gui/src/ui_runner/launcher_scheduler.rs:248` only tests construction-ish
behavior, but the scheduler owns catalog, media, and launch handoff workers.

Add tests around `launcher_scheduler.rs:73` through `launcher_scheduler.rs:154`
that prove:

- media gates pause during launch,
- queued media resumes after launch failure recovery,
- stale launch worker messages are dropped or ignored,
- catalog validation cannot start a foreground user-visible rebuild while the
  launch/recovery path owns the screen.

### P1: Input Hotplug And Event Boundaries

`magik-gui/src/input.rs:174` and `input.rs:228` are hard to test because they
own concrete `File` reads and `/dev/input` discovery.

Add a synthetic js-event reader boundary and tests for:

- `JS_EVENT_INIT` masking,
- button release,
- axis neutral/deadzone boundaries,
- hat-axis release,
- short reads,
- disconnect removal,
- active-pad raw/debug propagation.

Add controller setup tests for target pad unplug/disappearance so setup does
not silently stall (`magik-gui/src/ui_runner/controller_setup_input_session.rs:21`).

### P2: Route And Direct-Video Plan Tests

`magik-gui/src/fpga.rs:363` has little pure coverage for route math. Extract a
pure `FramebufferRoutePlan` or similar and test:

- scan dimensions,
- right-edge guard,
- stride,
- RGB565 flags,
- `set_vga_fb` behavior for direct-video versus HDMI.

### P2: Preview Pack/Index Failure Coherence

`magik-gui/src/ui_runner/media_worker.rs:685` installs the pack, then
`media_worker.rs:697` installs the index, then `media_worker.rs:708` writes
state. Add a test for "pack succeeds, index install fails" that verifies:

- existing destination pack/index/state remain coherent,
- state does not mark the new pair current unless both required artifacts are
  valid,
- preview metadata cache is not invalidated into an inconsistent state.

### P2: Corrupt Preview Payload End-To-End

Add tests where a valid archive contains malformed LZ4 or malformed raw565
payload:

- loader returns `PreviewResult { image: None }`,
- visible existing preview is preserved or blanked according to selected state,
- failure is cached at the preview layer,
- archive metadata cache is not poisoned.

### P2: Visual Rendering Contracts

Add small RGB565 signature tests for:

- normal Arcade list rows,
- filter drawer rows,
- search result geometry,
- production fade midpoint/end pixels,
- no stale/empty preview artifacts through `ui_frame_target`.

Useful anchors:

- `magik-gui/src/arcade_list_renderer.rs:257`,
- `magik-gui/src/ui_runner/ui_frame_target.rs:128`,
- `magik-gui/src/screenshot_transitions.rs:350`.

### P2: Frame-Budget Unit Checks

The device benchmark suite owns final performance evidence, but unit tests can
still protect expected row-copy behavior. Add tests for copied/direct row counts
on common frames:

- idle Arcade,
- one-row scroll,
- full-frame after modal,
- direct preview repaint after Slint dirty.

Anchor: `magik-gui/src/ui_runner/launcher_loop.rs:2093`.

### P3: Video Player Unit Coverage

`magik-gui/src/video_player.rs` has no local unit tests. Add tests around:

- RGB565 frame extraction,
- EOF and rewind behavior,
- unsupported pixel format errors.

## Structural Recommendations

1. Separate "unique behavioral test count" from Cargo's target count. Because
   lib and binary targets both compile some modules, test totals can look more
   impressive than the unique behavior coverage actually is.
2. Keep adding state-machine tests near lifecycle/session modules. That style
   is the best part of the current suite.
3. For hardware-in-the-loop (HIL) code, extract tiny pure planning functions and
   fakeable IO traits. Avoid trying to mock the whole MiSTer; mock only the
   operation sequence that must never regress.
4. Promote optional private-fixture checks into either explicit ignored tests or
   deterministic synthetic fixtures. Silent skips are easy to overvalue.
5. For future AI-authored tests, require every new test to name the protected
   user failure in the test name or setup. The best current tests already do
   this.

## Suggested Implementation Order

1. Fix and test stale launch event guards.
2. Add the launch failure loop-level recovery scenario.
3. Add the `ui_boot` fakeable startup-order harness.
4. Add preview failed-cache invalidation after media publish.
5. Add the production-shaped catalog first-scan fixture.
6. Add scheduler interleaving tests.
7. Fill catalog media-pruning parity and input hotplug boundaries.
8. Add visual/pixel and frame-budget tests for renderer quality.

## Implementation Follow-Up Checklist

Implementation bug report:
`docs/2026-07-02-test-audit-implementation-bugs.md`.

Every future AI-authored test should name the protected user failure or product
invariant in the test name, setup, or assertion message.

| Finding | Status | Coverage / Commit |
| --- | --- | --- |
| Stale launch lifecycle terminal events could affect non-launch states | Done | `314c5a7` adds lifecycle guards and tests for idle, catalog validation, recovery, and startup-like non-launch states. |
| Brittle lifecycle capacity assertion | Done | `d6e64f5` replaces capacity coupling with semantic transition/effect assertions. |
| Preview failed-cache survives media refresh | Done | `ec64978` clears failed preview paths when media worker reports current/downloaded packs and tests the cache/effect path. |
| HIL skips counted as passes | Done | `bf901fe` adds `record_skip`, `results.tsv`, `summary.json`, result table output, and host-tool contract checks. |
| Loop-level launch failure journey | Planned | Add host loop/session test for loading frame, handoff failure, recovery frame, idle return, input/list/preview resumption. |
| Launch/recovery frame-policy suppression | Planned | Add tests proving preview scheduling/apply and Arcade overlay drawing are suppressed while launching. |
| Scheduler interleaving for catalog/media/launch workers | Planned | Extend `launcher_scheduler` and session tests for media pause/resume/drop, catalog validation visibility, and stale launch messages at scheduler boundaries. |
| Boot/framebuffer startup order | Planned | Extract fakeable startup plan for geometry, RGB565, black clear, display config, FPGA route, and settle frames. |
| FPGA route/direct-video planning | Planned | Extract route plan tests for scan dimensions, right-edge guard, stride, RGB565 flags, HDMI/direct-video `set_vga_fb`, and route-invalid composition recovery. |
| Input and controller setup boundaries | Partially done | Added synthetic JS event drain tests for `JS_EVENT_INIT` masking, button release, hat-axis release, EOF disconnect, and short-read disconnect. Controller setup target-pad unplug/disappearance remains planned. |
| Button override write/remove behavior | Partially done | Added temp-path IO tests for actual override file writes and empty override-set stale file removal. Launcher-level non-MRA removal is still covered by fake launch IO; broader launch-policy clobber tests remain planned. |
| Production-shaped catalog fixture | Planned | Add end-to-end fixture across `_Arcade`, games, DOS, generic cores, helper/media clutter, loose files, zip entries, MGL launchers, RAM catalog, SQLite save/load, and corrupt zip resilience. |
| Catalog pruning and audit parity | Planned | Add media/helper directory fixtures, mixed-case variants, `core_audit` parity, MGL duplicate suppression, and supported-input cache decision test. |
| Optional and brittle catalog tests | Planned | Convert private TSV fixture to ignored or explicit skipped-fixture test; prefer structured progress assertions over exact legacy copy. |
| Preview state hot-path coverage | Planned | Introduce preview-loader trait/test double; prove selected cache misses do not synchronously block and queued media systems do not download during active scroll interaction. |
| Preview pack/index coherence | Planned | Add pack-success/index-failure and corrupt-payload tests that preserve pack/index/state/cache coherence. |
| RGB565/archive validation alignment | Planned | Add contract tests comparing raw565 header/stride rules with preview worker V2 acceptance, including padding and oversize guards. |
| Video player unit coverage | Planned | Add host-only/feature-gated tests for RGB565 extraction, EOF/rewind, and unsupported pixel format errors. |
| Visual rendering quality contracts | Planned | Add RGB565 signature tests for Arcade rows, filter/search geometry, fades, and no stale preview artifacts. |
| Frame-budget unit checks | Planned | Add lightweight row-count/direct-row tests for idle Arcade, one-row scroll, modal full-frame, and direct preview repaint. |
| HIL audit-risk scenarios | Planned | Add device scenarios for invalid launch target recovery, same-process preview availability after pack/index refresh, startup reveal surfacing, and catalog helper/media decoys. |
