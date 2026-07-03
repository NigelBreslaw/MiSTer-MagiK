# MiSTer MagiK Performance Review - 2026-07-03

This report covers production code only. Experimental effects were intentionally
excluded. Phase one was a static review of the current codebase using three
focused explorer agents. Phase two ran production real-hardware benchmarks on
the reference MiSTer at `192.168.1.117`.

## Executive Summary

The production architecture is directionally right for the dual-core Cortex-A9
and exFAT SD card:

- Slint renders into cached RGB565 RAM.
- The frame loop copies dirty regions into `/dev/fb0`.
- Arcade list and screenshot preview are Rust-painted RGB565 overlays.
- First catalog creation runs foreground on both cores until the RAM catalog is
  usable.
- Warm/background catalog, media, and preview prefetch work are isolated with
  lower priority and CPU0 affinity.
- Preview cold paths can use `.idx` sidecars and `pread` without reading whole
  screenshot packs.

The biggest remaining opportunities are not in raw framebuffer copy alone. The
hardware evidence points to four deeper optimization tracks:

1. Make preview scheduling and cache lookup cheaper in the frame loop.
2. Avoid archive-memory preview loads in steady warmed runs when sidecar indexes
   already exist.
3. Reduce first-scan discovery/classification work and avoid extra exFAT reads
   after publishing SQLite.
4. Keep Slint dirtiness and full-frame cached presents away from Arcade overlay
   frames.

The CPU flamegraph run could not complete because the profiling build needed
network downloads and the host network was failing due to a stale VPN-related
issue. The production trace attribution still gave useful phase-level evidence.

## Phase One - Static Review

### Render And Frame Loop

Relevant modules:

- `magik-gui/src/ui_runner/launcher_loop.rs`
- `magik-gui/src/ui_runner/ui_frame_target.rs`
- `magik-gui/src/ui_runner/raw565_preview_renderer.rs`
- `magik-gui/src/arcade_list_renderer.rs`
- `magik-gui/src/framebuffer/target.rs`
- `magik-gui/src/framebuffer/mapped.rs`

Findings:

- The cached-RAM RGB565 render path is the right production shape. Direct Slint
  framebuffer rendering should stay out of production.
- Arcade scrolling reuses a RAM ring surface, but scroll frames still present
  the full `464x384` list overlay. Measured scroll-frame cost is only about
  `0.60-0.71ms`, so this is useful but not the first optimization to chase.
- Direct preview uses a full `960x540` backing buffer even though the preview
  box is about `320x320`. A rect-local direct preview target would improve cache
  locality and reduce memory touched during composition.
- Preview scheduling does more work than ideal per frame: it builds window keys,
  uses owned string collections, and does cache retention before the fastest
  same-selection exit.
- Slow-frame attribution frequently points at `preview_schedule_us` and preview
  apply work, not at framebuffer present.
- Search index prewarm after first visible copy can create a post-startup spike
  and should be sliced/backgrounded if it appears in startup traces.

### Catalog, SQLite, And exFAT

Relevant modules:

- `magik-gui/src/ui_runner/catalog_worker.rs`
- `magik-gui/catalog/src/library_db.rs`
- `magik-gui/catalog/src/catalog_scan.rs`
- `magik-gui/catalog/src/catalog_navigation.rs`
- `magik-gui/catalog/src/preview_worker.rs`
- `magik-gui/catalog/src/sqlite_catalog.rs`

Findings:

- First-scan correctly sends `Ready` from the RAM catalog before durable save.
- SQLite import builds in tmpfs and publishes the final DB to exFAT, which is
  good. Publishing a 10.3MB DB measured under 1s.
- The projection save path can reread the just-published database from
  `/media/fat` to build projections. That is avoidable exFAT work; derive from
  the tmpfs build DB or the same SQLite-materialized rows instead.
- Warm checkpoint validation is cheap enough on this card, but it still walks
  core roots and top-level game dirs. Messy cards with many unknown dirs could
  benefit from cached payloadish status keyed by parent metadata.
- Preview sidecar metadata has short TTL/stat behavior and may re-read media
  state. Structurally valid `.idx` files should be trusted more aggressively
  after media publish.
- If sidecar lookup misses, warmed preview flows can fall back to full
  `.mmlz4b` archive memory loads. Cold first-preview and cold turbo gates show
  that `index_pread` works well; production should bias harder toward that path.

### Runtime Scheduling

Relevant module:

- `magik-gui/catalog/src/runtime_thread.rs`

Findings:

- Foreground first catalog build uses unrestricted CPU and nice `0`.
- Warm validation, library walker, media worker, media index, and preview
  prefetch are correctly deprioritized or pinned to CPU0.
- Selected preview stays interactive priority. That is sensible, but selected
  misses should avoid synchronous UI-thread decoding/lookup where possible.

## Phase Two - Hardware Evidence

Current deployed binary:

- Profile: `release-device`
- Features: `ui,bench-tools`
- Size: `6,295,460` bytes
- Target: RGB565 `960x540`, FPGA-scaled to HDMI

Catalog acceptance passed before and after the disruptive first-scan run:

- One launcher process.
- No active `library-refresh`.
- SQLite catalog present.
- `arcade/neogeo/saturn` preview coverage populated.
- No stale reset-fault arming files found after benchmarks.

### Benchmark Results

| Area | Label | Result |
| --- | --- | --- |
| Launcher scene | `P2-20260703-LAUNCHER` | 60fps, render `30us`, present `1644us`, rows `540`, visual/timing/capture OK |
| Arcade turbo scroll | `P2-20260703-ARCADE-TURBO` | Vsync pass, no fallback/timeout/error, max miss streak `0` |
| Preview turbo scroll | `P2-20260703-PREVIEW-TURBO` | Valid trace; p99 work `16557us`; 16 work frames over 16.7ms |
| Preview 60fps gate held | `P2-20260703-PREVIEW-GATE-FADE-VEL` | Passed; p99 work `14189us`; 1 work frame over 16.7ms |
| Preview 60fps gate turbo | `P2-20260703-PREVIEW-GATE-FADE-TURBO` | Passed; p99 work `14406us`; 1 work frame over 16.7ms |
| First preview | `P2-20260703-FIRST-PREVIEW` | First selected preview from `index_pread`; apply age `15000us`; decode total `9322us` |
| Cold turbo preview | `P2-20260703-COLD-TURBO` | `arcade`, `neogeo`, `saturn` all passed; zero blank/stale/miss; zero archive memory loads |
| Library IO | `P2-20260703-LIB-IO` | Cold-ish scan `28.2s`; classify `27.9s`; import `14.4s`; publish `0.77s` |
| Library save | `P2-20260703-LIB-SAVE` | Publish iterations `0.68-0.75s`; warm scan/classify around `5.0-5.3s`; import around `13.0s` |
| First scan | `P2-20260703-FIRST-SCAN` | `library_ready=49045ms`; `library_db_saved=69891ms`; both pass gates |

### Important Trace Details

Preview turbo steady run:

- `frames_after_30=1640`
- `p95_work_us=15130`
- `p99_work_us=16557`
- `work_gt_16_7ms=16`
- `vsync_source_vsync=1640`
- `fallback=0`, `timeout=0`, `error=0`, `max_miss_streak=0`
- Slow-frame attribution: `preview=11`, `dominant_prepare=4`,
  `dominant_slint_render=1`.
- `preview_schedule_us` appears in many slow frames around `12-19ms`.
- Preview timing had `archive_mem=549`, `decoded_cache=107`,
  `index_pread=11`.

Preview gate held/turbo:

- Held gate p99 work `14189us`, turbo gate p99 work `14406us`.
- Both pass the `14500us` p99 threshold.
- Both had one dominant Slint-render spike around frame 100.
- Both maintained perfect vsync source accounting.

Cold preview:

- First preview used `index_pread`.
- Cold turbo:
  - `arcade`: first request to apply `31ms`, max `222ms`, `23` index preads.
  - `neogeo`: first request to apply `10ms`, max `13321ms`, `6` index preads.
  - `saturn`: first request to apply `15ms`, max `21619ms`, `2` index preads.
- No visible miss occurred despite the high max request-to-apply on neogeo and
  saturn. Investigate these long tails before assuming they are harmless.

First scan:

- `library_scan_complete=44423ms`
- `library_ready=49045ms`, under the `57094ms` gate
- `library_db_saved=69891ms`, under the `72573ms` gate
- `scan_stage_walk=42483ms`
- `scan_stage_file_discovery=31205ms`
- `scan_stage_classify_total=43141ms`
- RAM catalog projection `catalog_us=4542280us`
- SQLite import `14916870us`
- SQLite publish `732ms`
- Final `db_count=13205`

## Optimization Backlog

### 1. Move Preview Scheduling Work Out Of Hot Frames

Strength: Strong

Problem:

The steady turbo trace passes the gate but slow frames are mostly preview
scheduling/apply work. `preview_schedule_us` can consume almost an entire frame.

Direction:

- Return before recomputing prefetch windows when selection and preview state
  are unchanged.
- Reuse scratch buffers for preview window keys.
- Avoid per-frame `HashSet`/`Vec<String>` churn.
- Represent preview keys with borrowed or interned identifiers where possible.
- Keep stale/exact preview visible while worker results arrive.

Proof:

- `profile-preview-scroll.sh LABEL --skip-build --secs 30 --scenario turbo-hold --visual-captures 0`
- Target movement: lower `preview_schedule_us`, fewer `work_gt_16_7ms`, p99 work
  comfortably below `14500us`.

### 2. Prefer Index-Pread Over Archive-Memory Warm Loads

Strength: Strong

Problem:

Cold first-preview and cold turbo prove `index_pread` is fast and reliable, but
steady warmed preview runs still show hundreds of `archive_mem` loads.

Direction:

- Treat valid `.idx` sidecars as the primary production path.
- Avoid full archive memory load unless explicitly requested by a warm benchmark
  or recovery path.
- Cache sidecar/media-state trust by pack fingerprint so repeated stat/JSON
  reads do not creep into browsing.

Proof:

- Cold/turbo gates must keep `archive_mem_loads=0`.
- Warm preview scroll should reduce `archive_mem` without increasing
  `unexpected_file_reads`, `slow_reads`, blank, stale, or failed previews.

### 3. Derive Projections Before exFAT Publish Or From tmpfs SQLite

Strength: Strong

Problem:

Publishing the DB to exFAT is cheap, but rereading final SQLite from
`/media/fat` to build projections is avoidable and risks SD-card variance.

Direction:

- Keep SQLite as the source of final ordering, but build summary/navigation
  projections from the tmpfs build DB before final publish.
- Alternatively retain materialized row data from import and write projections
  directly from that.

Proof:

- `profile-first-scan.sh LABEL --skip-build --replace-label --thread-sample`
- `profile-library-io.sh LABEL --replace-label`
- Watch `library_db_saved`, projection write timing, and any post-publish DB
  reads.

### 4. Reduce First-Scan Discovery/Classify Work

Strength: Strong

Problem:

First scan passes but most time is discovery/classification. The SD-card walk
and classification work dominate; final exFAT DB publish does not.

Direction:

- Avoid duplicate work between precount/bootstrap and full scan.
- Cache per-target signatures for large stable roots such as GBA, NES,
  MegaDrive, and Arcade.
- Consider selective parallelism only for first-build classification if trace
  data shows CPU headroom. Preserve current low-priority CPU0 policy for warm
  validation.
- Keep archive TOC header-only; do not decompress archives in scanner paths.

Proof:

- First-scan `library_ready` should move, not just `library_db_saved`.
- Thread samples should show foreground scan/classify using both cores only
  during first build.

### 5. Make Direct Preview Rect-Local

Strength: Worth exploring

Problem:

The direct preview backing buffer is full-frame sized. Copy/present is rect
limited, but composition touches a larger allocation and cache footprint than
the preview requires.

Direction:

- Store direct preview as a preview-rect surface with its own stride.
- Present it with strided rect copy.
- Keep visual capture tests around fade and exact/stale transitions.

Proof:

- `preview_blit_us`, `direct_preview_present_us`, RSS high-water, and visual
  captures.

### 6. Audit Slint Dirtiness Over Arcade

Strength: Worth exploring

Problem:

Idle/no-update frames often still copy 540 cached rows. That is acceptable, but
it is the largest repeated present cost in many traces.

Direction:

- Identify which bridge updates dirty the Arcade area during locked scroll.
- Keep Slint chrome stable while Rust overlays animate.
- Avoid full-frame presents around normal Arcade browsing.

Proof:

- `launcher-present-trace.py summarize`
- Lower `rows` on no-update frames without composition recovery.

### 7. Test New-Band-Only Arcade Scroll Present

Strength: Speculative

Problem:

Scroll frames currently copy the whole list overlay. Measured cost is modest,
but it is still a predictable per-scroll cost.

Direction:

- Prototype a strict new-band-only present path that relies on existing
  framebuffer contents for scrolled rows.
- Validate heavily because framebuffer reads are known bad and stale pixels
  would be easy to introduce.

Proof:

- `arcade_list_present_us` should drop materially.
- Visual captures must prove no stale rows, selection-frame damage, or modal
  recovery regressions.

## Artifacts

Key local artifacts:

- `history/toolchain-bench/results.tsv`
- `history/toolchain-bench/results-first-scan.tsv`
- `history/toolchain-bench/results-library-io.tsv`
- `history/toolchain-bench/results-library-save.tsv`
- `build/arcade-scroll-profiles/P2-20260703-ARCADE-TURBO-arcade-scroll.tsv`
- `build/preview-scroll-profiles/P2-20260703-PREVIEW-TURBO-arcade.tsv`
- `build/preview-scroll-profiles/P2-20260703-PREVIEW-GATE-FADE-VEL-arcade.tsv`
- `build/preview-scroll-profiles/P2-20260703-PREVIEW-GATE-FADE-TURBO-arcade.tsv`
- `build/preview-scroll-profiles/P2-20260703-FIRST-PREVIEW-arcade.tsv`
- `build/first-scan-profiles/P2-20260703-FIRST-SCAN-first-scan-thread-sample.tsv`

Commands that completed:

```bash
scripts/deploy-rust.sh --device --ui-scope launcher --bench-tools
scripts/device-catalog-acceptance.sh
scripts/bench-toolchain.sh P2-20260703-LAUNCHER --skip-build --replace-label --scene launcher --scene-secs 15
scripts/profile-arcade-scroll.sh P2-20260703-ARCADE-TURBO --secs 30 --scenario turbo-hold --skip-build --thread-sample
scripts/profile-preview-scroll.sh P2-20260703-PREVIEW-TURBO --skip-build --secs 30 --scenario turbo-hold --visual-captures 0 --thread-sample --replace-label
scripts/gate-preview-60fps.sh P2-20260703-PREVIEW-GATE --skip-build --visual-captures 0 --secs 30
scripts/profile-first-preview.sh P2-20260703-FIRST-PREVIEW --skip-build --replace-label
scripts/gate-cold-turbo-preview.sh P2-20260703-COLD-TURBO --systems arcade,neogeo,saturn --secs 10
scripts/profile-library-io.sh P2-20260703-LIB-IO --replace-label
scripts/profile-library-save.sh P2-20260703-LIB-SAVE --iterations 5 --replace-label
scripts/profile-first-scan.sh P2-20260703-FIRST-SCAN --skip-build --replace-label --thread-sample --timeout 240
```

Commands intentionally not completed:

- `scripts/profile-preview-scroll.sh P2-20260703-PREVIEW-CPU --cpu-profile ...`
  failed before deploying because Cargo could not download profile-only crates
  while the host network was unhealthy.
- `scripts/profile-preview-index-refresh.sh P2-20260703-IDX` requires a
  diagnostics-capable binary, so it was skipped for this production-only pass.
