# Production Performance Review Phase One - 2026-06-29

Scope: current production code only. Experimental effects are excluded. This is
the static review baseline before the next real-hardware benchmark pass.

## Executive Summary

The production architecture is fundamentally sound. The launcher already uses
the right big moves for a dual-core Cortex-A9 and exFAT SD-card target:

- Slint renders into cached RGB565 RAM, not directly into live framebuffer
  memory.
- `/dev/fb0` is treated as a write-only, write-combined target.
- Arcade scrolling uses a Rust-painted RGB565 list surface instead of a Slint
  list.
- Preview assets use raw565 `.mmlz4b` packs and `.idx` sidecars rather than
  runtime PNG/JPEG decode.
- Catalog builds use tmpfs for SQLite construction and publish with large
  sequential writes to `/media/fat`.

The best next optimizations are not exotic compiler flags or direct framebuffer
rewrites. They are:

1. Make background work visibly scheduler-aware on a 2-core system.
2. Eliminate surprising `/media/fat` writes from read-looking warm paths.
3. Reduce steady scroll copy volume or make the copy shape friendlier to the
   framebuffer mapping.
4. Measure core placement, context switches, and I/O contention alongside frame
   traces.

## Highest-Leverage Findings

### 1. Arcade Scroll Still Copies The Full List Overlay

The arcade renderer reuses a circular RAM surface, but `Scroll` updates still
present the full `464x384` list overlay to the framebuffer. That is deliberate:
reading or scrolling live `/dev/fb0` was measured slower on MiSTer's
write-combined framebuffer. The remaining question is copy shape.

Relevant code:

- `magik-gui/src/arcade_list_renderer.rs`
  - `copy_layer_to_target`
  - `copy_viewport_band_to_target`
  - selection-frame segmented presents
- `magik-gui/src/ui_runner/ui_frame_target.rs`
  - `copy_arcade_list_update`
- `magik-gui/src/ui_runner/launcher_compositor.rs`
  - cached/direct overlay present order

Optimization candidates:

- Benchmark one dense list copy including the selection frame against the
  current segmented frame-preserving copy.
- Benchmark fewer, wider copy segments for wrapped circular-surface cases.
- Keep any live-fb scroll-present revival out of production unless a new device
  run overturns the old write-combined evidence.

Risk: low to medium. Visual correctness around the selection frame is the main
failure mode.

### 2. Slint Dirty Intersections Force Arcade Overlay Work

If Slint marks a dirty rect intersecting the Rust-painted list viewport, the
arcade renderer forces a redraw/present of the overlay. This is safe, but it
means unrelated bridge-property churn can amplify into framebuffer work during
scroll.

Relevant code:

- `magik-gui/src/ui_runner/ui_frame_target.rs`
  - `arcade_list_needs_forced_redraw`
- `magik-gui/src/ui_runner/launcher_bridge.rs`
  - bridge sync paths
- `magik-gui/ui/arcade_list.slint`
  - static list viewport/chrome

Optimization candidates:

- Guard unchanged Slint property writes in the light bridge path.
- Keep loading/search/status updates from touching the arcade list viewport
  during steady scroll.
- Use present traces to confirm `slint_dirty` no longer intersects the list
  except on real chrome changes.

Risk: low. This should be benchmarked with `arcade_list_update_us`,
`cached_present_us`, and copied rows.

### 3. Preview Threads Use The Second Core, But Pressure Is Not Measured

Preview selected and prefetch loaders are separate background threads sharing a
decoded cache. Catalog validation/build and media downloads also run in
background workers. This is the right broad shape for dual-core Cortex-A9, but
the current benchmark artifacts do not show thread/core residency, migrations,
run-queue pressure, or context-switch bursts.

Relevant code:

- `magik-gui/catalog/src/preview_worker.rs`
  - `preview-selected-loader`
  - `preview-prefetch-loader`
  - nice `5`
- `magik-gui/src/ui_runner/catalog_worker.rs`
  - catalog worker nice `10`
- `magik-gui/src/ui_runner/media_worker.rs`
  - media worker starts at default priority
- `magik-gui/src/cpu_profile.rs`
  - in-process sampling profile only

Optimization candidates:

- Add a script-only sampler during benchmarks for
  `/proc/<pid>/task/*/{stat,status,sched}`.
- Compare selected preview latency with prefetch at nice `5`, nice `10`, and
  temporarily disabled prefetch.
- Lower media worker priority or make media activity defer more aggressively
  during Arcade interaction.

Risk: medium. Lowering selected-preview priority could hurt first-preview
latency; affinity can backfire on a small SMP system.

### 4. Warm Catalog Reads Can Repair Navigation Projections

`load_arcade_catalog_from_sqlite` opens the database read-only, but after load
it may repair the navigation projection. If the projection is missing or stale,
that write path syncs and renames a file under `/media/fat`. A read-looking warm
path can therefore become an exFAT write/sync path.

Relevant code:

- `magik-gui/catalog/src/sqlite_catalog.rs`
  - `load_arcade_catalog_from_sqlite_at`
  - `repair_navigation_projection_after_sqlite_load`
- `magik-gui/catalog/src/catalog_navigation.rs`
  - projection write, sync, rename, parent sync

Optimization candidates:

- Make SQLite load side-effect-free.
- Treat projection repair as an explicit background maintenance job after the
  UI is already usable.
- Prefer build/publish responsibility for projections, with runtime repair only
  as an opt-in diagnostic path.

Risk: medium-high for warm startup consistency on missing/stale projection
devices.

### 5. Media Publish Uses External `sync`

Screenshot/media artifact publish uses `sync <path>` and falls back to plain
`sync`. On BusyBox/exFAT this may become a global flush, which can stall
interaction and hide unrelated SD-card work under a media update.

Relevant code:

- `magik-gui/src/artifact_publish.rs`
  - `sync_path_with_fallback`
  - `sync_path_best_effort`
- `magik-gui/src/ui_runner/media_worker.rs`
  - pack install and media-state writes

Optimization candidates:

- Use file `sync_all` and parent-directory `sync_all` like SQLite publish.
- Avoid shelling out for sync in the runtime process.
- Keep runtime media concurrency at one by default; benchmark any higher value
  only while also tracing frame pacing and I/O wait.

Risk: medium-high during media update or cold media boot.

### 6. Preview Cache Hits Still Re-Stat Archives

Cached preview archive lookup recomputes archive fingerprints with
`metadata()`. On exFAT/FUSE, metadata calls during fast preview scroll can add
jitter even when image decode is a cache hit.

Relevant code:

- `magik-gui/catalog/src/preview_worker.rs`
  - cached archive lookup/fingerprint comparison
  - sidecar `index_pread`
  - full archive background warm

Optimization candidates:

- Trust an opened archive until a media worker generation changes.
- Add a short metadata TTL.
- Keep `index_pread` mode longer for rarely used systems instead of immediately
  warming whole packs.

Risk: medium. Correctness depends on handling media replacement events cleanly.

### 7. Build Profile Is Strong; Benchmark Scope Needs Labels

The release-device profile already uses `opt-level=3`, `panic=abort`, stripped
binary, fat LTO, one codegen unit, `target-cpu=cortex-a9`, and NEON. Device
Slint is built without `std`.

The main risk is comparing artifacts with different UI scopes. Production
deployable builds default to all UI scope; some benchmark paths use launcher
scope. Those are both valid, but reports should label them distinctly.

Relevant code:

- `magik-gui/Cargo.toml`
- `magik-gui/build-arm.sh`
- `scripts/bench-toolchain.sh`
- `magik-gui/BUILD.md`

Optimization candidates:

- Report binary/build metrics as `prod-all`, `launcher-scope`, or
  `video/all-scenes`.
- Do not swap allocators unless allocation traces show a real runtime cost.

Risk: low for labeling, high for allocator experiments without long soak.

## Phase Two Benchmark Plan

Run production benchmarks only:

1. Device health and deployed binary status
   - `scripts/mister status`
   - `scripts/mister db "SELECT count(*) FROM games"`

2. Warm startup and catalog hydration
   - `scripts/profile-warm-catalog-start.sh PERF20260629-WARM --replace-label --iterations 5`

3. Arcade steady scroll
   - `scripts/profile-arcade-scroll.sh PERF20260629-ARCADE --secs 30 --scenario turbo-hold --skip-build`

4. Preview steady scroll
   - `scripts/profile-preview-scroll.sh PERF20260629-PREVIEW --secs 30 --scenario turbo-hold --skip-build --visual-captures 0`

5. Preview cold/index lane
   - `scripts/profile-first-preview.sh PERF20260629-FIRST-PREVIEW --skip-build`
   - `scripts/profile-preview-scroll.sh PERF20260629-PREVIEW-COLD --secs 30 --scenario turbo-hold --skip-build --skip-preview-warm --visual-captures 0`

6. Release gate
   - `scripts/gate-preview-60fps.sh PERF20260629-GATE --skip-build --visual-captures 0`

7. Catalog/storage
   - `scripts/profile-library-save.sh PERF20260629-LIBSAVE --iterations 5 --replace-label`
   - `scripts/profile-library-io.sh PERF20260629-LIBIO --replace-label`
   - `scripts/profile-first-scan.sh PERF20260629-FIRSTSCAN --skip-build --replace-label`

8. Media update/storage
   - `scripts/profile-screenshot-save.sh PERF20260629-SAVE --system neogeo --iterations 10 --replace-label`
   - `scripts/profile-media-cold-boot.sh PERF20260629-MEDIA-COLD --skip-build --replace-label`

9. Launch handoff
   - `scripts/profile-launch-handoff.sh PERF20260629-LAUNCH --replace-label --iterations 5`
   - `scripts/profile-launch-prep.sh PERF20260629-LAUNCH-WARM --replace-label --scenario warm --iterations 5`
   - `scripts/profile-launch-prep.sh PERF20260629-LAUNCH-COLD --replace-label --scenario cold --iterations 3`

Optional script-only enhancement for this benchmark pass:

- Sample `/proc/<pid>/task/*/{stat,status,sched}` during Arcade and preview
  runs to correlate slow frames with per-thread CPU/core movement.

## Initial Optimization Backlog

1. Make SQLite catalog load side-effect-free; move nav projection repair out of
   warm read paths.
2. Replace runtime shell `sync` calls with file/parent directory `sync_all`.
3. Add benchmark thread sampler for Cortex-A9 core pressure.
4. Benchmark dense arcade list present vs current segmented selection-preserving
   present.
5. Add frame-pressure-aware preview prefetch throttling.
6. Add archive metadata TTL or media-generation invalidation for preview cache
   hits.
7. Label benchmark artifact scope consistently.

