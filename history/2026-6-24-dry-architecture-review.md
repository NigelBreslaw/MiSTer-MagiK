# Deep DRY Architecture Review - 2026-06-24

This report reconstructs the deep DRY review after the original temporary HTML
artifact became unavailable. The review looked for repeated concepts that are
still shallow in the codebase, with a bias toward refactors that preserve the
launcher hot path and avoid extra per-frame allocation, copies, filesystem
work, or device round trips.

## Executive Summary

The best first move is to deepen the launcher bridge presenters. The biggest
duplication pressure in the project is not repeated text, but repeated knowledge
about UI state, invalidation, worker side effects, and frame timing leaking
through `launcher_loop.rs`, `launcher_bridge.rs`, and the Slint bridge globals.

Do not start by extracting small helper functions. The valuable DRY work here is
to move ownership of a repeated rule set behind narrower modules:

- what changed this frame
- which Slint properties must be touched
- which work is allowed on the frame path
- how background worker events become bounded UI intents
- how preview/media artifacts are identified, validated, and published

No production code changes were made as part of this review.

## Priority Map

| Priority | Candidate | Main Risk It Removes | Performance Constraint |
| --- | --- | --- | --- |
| P1 | Deepen launcher bridge presenters | UI state and invalidation rules spread across the frame loop | Keep light-path updates allocation-light and property-specific |
| P1 | Make preview surface deep | Preview geometry, invalidity, stride, and transition handling repeated | Avoid new frame copies and per-item decode work |
| P2 | Unify screenshot media identity | Screenshot/cache path and identity rules repeated across catalog/media | Keep catalog projection path-only, no hot-path stat storms |
| P2 | Hide launcher control protocol | Scripts repeat restart/env/cleanup protocol details | Preserve stable script entrypoints and current benchmark behavior |
| P2 | Extract durable artifact publish | Copy/fsync/rename durability repeated with small variations | Avoid adding extra syncs on exFAT/FUSE |
| P2 | Split launch preparation | Launch ref materialization and cache stamping buried in launcher code | Preserve cached lookup behavior and Main handoff timing |
| P3 | Deepen launcher background work | Workers leak lifecycle/progress/reset details into frame loop | Bound work consumed per frame |
| P3 | Unify catalog projection and progress | SQL, Rust projection, and progress strings encode overlapping rules | Keep SQLite materialized tables for speed |
| P3 | Deepen device workflow modules | Scripts and host tools share transaction/recovery logic informally | Keep direct commands and approved wrappers intact |
| P3 | Collapse effect showcase shells | Experimental effect loops repeat timing and scene boilerplate | Keep production UI untouched |

## 1. Deepen Launcher Bridge Presenters

Files:

- `magik-gui/src/ui_runner/launcher_bridge.rs`
- `magik-gui/src/ui_runner/launcher_loop.rs`
- `magik-gui/ui/mister_bridge.slint`

What is duplicated:

- The frame loop knows which Slint properties correspond to selection,
  favorites, tabs, status, setup state, preview readiness, and benchmark labels.
- `launcher_bridge.rs` is currently a useful boundary, but the interface is
  still broad enough that callers own too much invalidation policy.
- Several update paths decide independently whether to do a full sync, a light
  sync, or a property-specific write.

Deeper refactor:

Introduce presenter-style modules behind the existing bridge. For example:

- `SelectionPresenter`
- `PreviewPresenter`
- `StatusPresenter`
- `SetupPresenter`
- `TabsPresenter`

Each presenter should accept typed state/events and produce a small bridge patch
or apply a bounded set of Slint property writes. The frame loop should stop
knowing the exact Slint property set for each domain.

Performance guardrails:

- Keep the current light sync path fast.
- Do not allocate new `SharedString` or `VecModel` values for unchanged fields.
- Track dirty keys or small generation counters inside presenters.
- Preserve the current single-frame ordering where visible state changes and
  framebuffer commits align.

Suggested first slice:

Move status/setup text and visibility updates behind one presenter first. This
is low risk compared with selection and preview updates, but it proves the
shape of the presenter API.

## 2. Make Preview Surface Deep

Files:

- `magik-gui/src/preview_state.rs`
- `magik-gui/src/ui_runner/ui_frame_target.rs`
- `magik-gui/src/arcade_list_renderer.rs`
- `magik-gui/ui/arcade_list.slint`

What is duplicated:

- Multiple callers interpret empty frames, RGB565 stride, dirty rectangles,
  transition IDs, cabinet/list geometry, and preview validity.
- Preview readiness is represented partly as optional buffers and partly as UI
  state.

Deeper refactor:

Make `PreviewSurface` the authority for:

- dimensions and stride
- pixel format
- frame validity
- transition identity
- dirty rectangles
- zero-copy read views
- geometry mapping used by arcade/list presentation

Performance guardrails:

- Do not introduce a conversion step between RGB565 buffers and UI copy paths.
- Keep view APIs borrowed where possible.
- Avoid per-frame heap allocation when only dirty rectangles changed.
- Keep existing benchmark scenarios for preview scroll and arcade scroll as the
  acceptance gate.

Suggested first slice:

Move preview frame validity and empty-frame interpretation into a single method
such as `PreviewSurface::frame_status()`, then update callers to stop checking
buffer length and dimensions directly.

## 3. Unify Screenshot Media Identity

Files:

- `magik-gui/src/media_update.rs`
- `magik-gui/catalog/src/preview_worker.rs`
- `magik-gui/catalog/src/library_db.rs`
- `magik-gui/catalog/src/software_identity.rs`
- `magik-gui/src/launcher.rs`

What is duplicated:

- Screenshot pack support repeats rules for software identity, expected paths,
  raw565 naming, media metadata, and size validation.
- Catalog code and runtime code both know pieces of screenshot/cache naming.

Deeper refactor:

Add a media identity module that owns:

- canonical screenshot key
- source screenshot path
- raw565 cache path
- snapshot pack path
- validation metadata
- human-readable mismatch diagnostics

Performance guardrails:

- Catalog projection should continue storing and passing paths, not opening
  media files on the hot path.
- The scanner must not walk screenshot/cache directories.
- Do not add metadata queries to selection movement or preview frame render.

Suggested first slice:

Replace repeated path construction with a typed `ScreenshotAssetId` and
`ScreenshotAssetPaths` pair. Leave actual loading behavior untouched.

## 4. Hide Launcher Control Protocol

Files:

- `scripts/profile-preview-scroll.sh`
- `scripts/profile-arcade-scroll.sh`
- `scripts/device-release-acceptance.sh`
- `scripts/run-rust.sh`
- `tools/mister/src/main.rs`

What is duplicated:

- Device scripts repeat process cleanup, launcher env variables, wait behavior,
  status checks, and failure diagnostics.
- The host tool knows some of this protocol, but not enough to let scripts stay
  declarative.

Deeper refactor:

Create a host-tool command or module that models launcher restart as a typed
operation:

- stop current Rust launcher
- set launch mode and benchmark env
- start command
- wait for ready state
- collect logs/status on failure

Performance guardrails:

- Keep existing scripts as stable public entrypoints.
- Avoid extra device round trips in benchmark loops.
- Preserve current labels and TSV output format for comparison continuity.

Suggested first slice:

Add one host command for "restart launcher for benchmark" and migrate one
profile script to use it.

## 5. Extract Durable Artifact Publish

Files:

- `magik-gui/src/media_pack_save.rs`
- `magik-gui/catalog/src/sqlite_catalog.rs`
- `magik-gui/src/ui_runner/media_worker.rs`
- `magik-gui/src/media_bench_save.rs`

What is duplicated:

- Several paths implement variants of temporary write, copy, rename, flush, and
  parent-directory durability.
- Error messages and cleanup behavior drift between SQLite, media packs, and
  benchmark artifacts.

Deeper refactor:

Add a small durable publish module for local artifact creation:

- write or copy to temp path
- flush the file when needed
- atomic rename where supported
- sync parent directory when required
- cleanup temp files on error
- standardize diagnostics

Performance guardrails:

- Do not add extra `fsync` calls to large media pack paths without measuring.
- Keep exFAT/FUSE behavior in mind; many small writes are slow.
- Let callers choose durability level explicitly.

Suggested first slice:

Extract the temp-path and rename policy first, with durability mode left as an
explicit option.

## 6. Split Launch Preparation

Files:

- `magik-gui/src/launcher.rs`
- `magik-gui/catalog/src/game_discovery.rs`
- `magik-gui/catalog/src/library_db.rs`
- `magik-gui/catalog/src/virtual_launch_cache.rs`

What is duplicated:

- Launch preparation mixes game identity, launch ref materialization, cache
  stamping, virtual launch handling, failure recovery, and Main handoff.
- Discovery and database code each know some launchability rules.

Deeper refactor:

Introduce a launch preparation module that owns the transition from catalog row
to launch request:

- canonical launch ref
- cache stamp validation
- virtual launch mapping
- user-facing failure reason
- Main handoff payload

Performance guardrails:

- Preserve cached lookup behavior.
- Do not add filesystem scans to the launch button path.
- Keep failure recovery fast enough to restore launcher display and input.

Suggested first slice:

Move launch ref materialization and diagnostic reason construction out of
`launcher.rs` while leaving the actual handoff call where it is.

## 7. Deepen Launcher Background Work

Files:

- `magik-gui/src/ui_runner/launcher_loop.rs`
- `magik-gui/src/ui_runner/catalog_worker.rs`
- `magik-gui/src/ui_runner/media_worker.rs`

What is duplicated:

- The frame loop knows worker lifecycle, progress messages, reset behavior,
  invalidation, and status display policy.
- Catalog and media workers each expose slightly different event shapes for
  concepts that the launcher consumes similarly.

Deeper refactor:

Route worker output through a `LauncherWorkQueue` or similar component that
converts worker messages into bounded UI intents:

- status update
- catalog changed
- media changed
- preview invalidated
- setup progress changed
- error surfaced

Performance guardrails:

- Bound the number of intents applied per frame.
- Avoid draining unbounded worker queues during a render frame.
- Keep progress updates coalesced.

Suggested first slice:

Normalize progress and status messages into typed events before touching the
larger catalog/media worker lifecycles.

## 8. Unify Catalog Projection And Progress

Files:

- `magik-gui/catalog/src/library_db.rs`
- `magik-gui/catalog/src/sqlite_catalog.rs`
- `magik-gui/src/ui_runner/catalog_worker.rs`

What is duplicated:

- Library projection rules live partly in Rust structs, partly in SQLite
  materialized queries, and partly in progress string generation.
- Progress reporting carries user-facing text rather than structured state in
  several places.

Deeper refactor:

Define canonical projection rows and structured progress events:

- projection row schema as Rust type plus SQL mapping
- stable progress phases
- counted work units
- final summary state

Performance guardrails:

- Keep SQLite materialized tables for query speed.
- Do not replace efficient SQL filtering with Rust-side filtering.
- Avoid emitting progress events for every row when chunked counts are enough.

Suggested first slice:

Introduce structured progress phases and convert to display text only at the UI
edge.

## 9. Deepen Device Workflow Modules

Files:

- `scripts/deploy-rust.sh`
- `tools/mister/src/main.rs`
- `tools/magik-agent/src/main.rs`
- `scripts/device-catalog-destruction.sh`
- `scripts/device-library-change-flow.sh`
- `magik-gui/build-arm.sh`

What is duplicated:

- Deploy, status, suspend/resume, probe, cleanup, and recovery logic are shared
  informally between scripts and host-tool commands.
- Scripts contain repeated assumptions about runtime layout and device state.

Deeper refactor:

Move stateful device workflows into host-tool modules while preserving script
entrypoints:

- deploy transaction
- launcher suspend/resume
- status probe
- recovery command
- catalog acceptance harness
- ARM build plan detection

Performance guardrails:

- Keep `scripts/mister` as the device communication wrapper.
- Avoid extra status probes in tight benchmark paths.
- Keep local ARM build backend behavior unchanged unless explicitly requested.

Suggested first slice:

Model deploy as a transaction in the host tool, then have
`scripts/deploy-rust.sh` call the transaction instead of repeating the protocol.

## 10. Collapse Effect Showcase Shells

Files:

- `magik-gui/src/effect_showcase.rs`
- `magik-gui/src/effect_controller.rs`
- `magik-gui/src/transition_effect.rs`
- `magik-gui/src/transition_render.rs`
- `scripts/experiments/effects/profile-text-effects.sh`

What is duplicated:

- Experimental effect scenes repeat loop timing, scene setup, and profiling
  shell behavior.
- The helper functions do not fully hide the scene protocol.

Deeper refactor:

Create a generic experimental showcase shell that owns:

- clock/tick behavior
- scenario setup
- frame count
- capture/profiling labels
- effect-specific state injection

Performance guardrails:

- Keep this isolated from production launcher paths.
- Do not make production transitions depend on experimental profiling code.

Suggested first slice:

Consolidate profiling script setup and labels first. Leave rendering internals
alone until a specific production transition needs the structure.

## Things I Would Not DRY Yet

- Do not combine all bridge updates into one generic property map. That would
  hide per-property cost and likely add allocation or dynamic dispatch to a
  frame-sensitive path.
- Do not replace SQL projection with a pure Rust projection layer just to reduce
  conceptual duplication. The current SQLite materialization exists for speed.
- Do not centralize all scripts behind a single mega command. Preserve clear
  benchmark and recovery entrypoints.
- Do not make preview/media validation perform filesystem checks during
  selection movement.
- Do not fold experimental effects into production launcher rendering unless a
  measured production use case needs it.

## Recommended Next Refactor

Start with status/setup presenter extraction inside the launcher bridge:

1. Add a small presenter that owns status text, setup overlay visibility, and
   progress display decisions.
2. Add tests for unchanged-state behavior if the bridge layer already has a
   host-testable boundary, or add a narrow unit around the presenter state.
3. Verify with `scripts/dev-rust check` and the launcher/preview benchmark
   entrypoints before expanding to selection and preview presenters.

This gives the project a deeper boundary where the duplication is most visible,
without touching preview copy paths, catalog scanning, or device deployment.
