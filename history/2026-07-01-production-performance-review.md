# Production Performance Review - 2026-07-01

Scope: production code only. Experimental effects, mega transitions, direct
Arcade scene paths, wider-color framebuffer paths, and row-step scroll
benchmarks are excluded.

Tree reviewed: `0aebd08c4 Make cold turbo previews zero-miss`.

Phase one was a static, multi-agent review of the current code. Phase two was
initially blocked because the MiSTer at `192.168.1.117` was not reachable:

```text
scripts/mister status
connection timed out

scripts/mister status  # rerun with network approval
Host is down (os error 64)

scripts/mister db "SELECT count(*) FROM games"  # rerun with network approval
Host is down (os error 64)
```

After the device was powered on, phase two continued on real hardware.

## Phase Two Benchmark Provenance

Device health before deployment:

- `/dev/fb0`: RGB565, `960x540`.
- `MiSTer_MagiK`: running.
- `mister-magik-fb`: running.
- Launcher: Home, about 60fps.
- Catalog query: `13205` games.

Deployed benchmark binary:

- Command:
  `scripts/deploy-rust.sh --device --ui-scope launcher --bench-tools`
- Profile/features: `release-device`, `ui,bench-tools`.
- Binary size: `6234020` bytes.
- Deploy checksum: `a5d571560d07dd97`.
- Deploy transport: Main-supervised MagiK agent deploy.

Many later scripts print `binary_scope=deployed-unknown` because they were run
with `--skip-build`, but they used this freshly deployed binary.

CPU profile note:

`scripts/profile-preview-scroll.sh PHASE2-20260701-PREVIEW-CPU --secs 30
--scenario turbo-hold --cpu-profile --visual-captures 0` was attempted, but the
profiling build stalled downloading profiler-only crates such as `backtrace`,
`addr2line`, `inferno`, and `gimli`. The stuck local build was stopped before
any profiling binary was deployed. The device was then checked healthy on the
launcher.

## Phase Two Results

### Warm Catalog Startup

Command:

```bash
scripts/profile-warm-catalog-start.sh PHASE2-20260701-WARM --replace-label --iterations 5
```

Rows appended to `history/toolchain-bench/results-warm-catalog.tsv`.

Result:

- All 5 iterations passed.
- Reveal/input: `38-43ms`.
- Full catalog hydration: about `2240-2274ms`.
- Stamp/checkpoint validation: about `358-369ms`.

Read:

Warm boot is healthy. Summary projection makes the first frame effectively
instant, while full SQLite hydration completes around 2.25s in the background.

### Arcade Scroll

Command:

```bash
scripts/profile-arcade-scroll.sh PHASE2-20260701-ARCADE --secs 30 --scenario turbo-hold --skip-build --thread-sample
```

Artifacts:

- `build/arcade-scroll-profiles/PHASE2-20260701-ARCADE-arcade-scroll.tsv`
- `build/arcade-scroll-profiles/PHASE2-20260701-ARCADE-arcade-scroll-thread-sample.tsv`

Result:

- Post-warm vsync path: `1589/1589` true vsync frames.
- Fallback/timeout/error: `0`.
- Max vsync miss streak: `0`.
- Steady scroll p99 `fb_present_us`: `1286`.
- Steady scroll p99 `arcade_list_present_us`: `729`.
- Rows p99: `704`.
- Composition recovery: `0`.

Read:

The steady Arcade list path is healthy. Startup/full-frame spikes exist, but
settled scroll is well inside budget. The full-list overlay copy remains a
future optimization candidate, not a current bottleneck.

### Preview Scroll

Commands:

```bash
scripts/profile-preview-scroll.sh PHASE2-20260701-PREVIEW --secs 30 --scenario turbo-hold --skip-build --visual-captures 0 --thread-sample
scripts/profile-preview-scroll.sh PHASE2-20260701-PREVIEW-COLD --secs 30 --scenario turbo-hold --skip-build --skip-preview-warm --visual-captures 0 --thread-sample
scripts/gate-preview-60fps.sh PHASE2-20260701-GATE --skip-build --visual-captures 0
```

Artifacts:

- `build/preview-scroll-profiles/PHASE2-20260701-PREVIEW-arcade.tsv`
- `build/preview-scroll-profiles/PHASE2-20260701-PREVIEW-COLD-arcade.tsv`
- `build/preview-scroll-profiles/PHASE2-20260701-GATE-FADE-VEL-arcade.tsv`
- `build/preview-scroll-profiles/PHASE2-20260701-GATE-FADE-TURBO-arcade.tsv`

Steady warmed preview:

- Valid trace, composition recovery `0`.
- Post-warm frames: `1672`.
- p99 work: `12935us`.
- Work frames over 16.7ms: `4`.
- All `1672` post-warm frames used true vsync.
- Max vsync miss streak: `0`.
- Preview slow-frame attribution: 2 preview-scheduling frames, 1 dominant
  prepare frame, 1 dominant custom-draw frame.
- Preview loads: `1084` rows, `487` archive-memory, `11` index-pread,
  `487` decoded-cache hits, unexpected file reads `0`.

Cold/no-warm preview:

- Valid trace, composition recovery `0`.
- Post-warm frames: `1724`.
- p99 work: `7225us`.
- Work frames over 16.7ms: `3`.
- All `1724` post-warm frames used true vsync.
- Max vsync miss streak: `0`.
- Preview loads: `1138` rows, `615` index-pread, `523` decoded-cache hits,
  archive-memory loads `0`, unexpected file reads `0`.

Release gate:

- Held-scroll p99 work: `5529us`.
- Held-scroll work frames over 16.7ms: `3`.
- Turbo p99 work: `6269us`.
- Turbo work frames over 16.7ms: `5`.
- Both gate halves: all true vsync, fallback/timeout/error `0`, max miss
  streak `0`.
- Gate result: passed.

Read:

Preview pacing is release-healthy. The important optimization target is not
steady throughput; it is rare preview scheduling/prepare spikes. The cold
index-pread path is doing its job and had better p99 work than the warmed
archive-memory path in this run.

### First Preview

Command:

```bash
scripts/profile-first-preview.sh PHASE2-20260701-FIRST-PREVIEW --skip-build
```

Artifact:

- `build/preview-scroll-profiles/PHASE2-20260701-FIRST-PREVIEW-arcade.tsv`

Result:

- Valid trace.
- Preview loads: `11`, all `index_pread`.
- Unexpected file reads: `0`.
- One post-warm work frame over 16.7ms, attributed to preview scheduling:
  about `20.2ms`.
- `first_preview_tsv` reported `decoded_seen=0` and `apply_seen=0`.

Read:

The index fast lane works, but the first-preview summary row did not observe a
selected decoded/apply event even though preview decode rows existed. Treat this
as an instrumentation/readiness gap to tighten before using `first_preview_tsv`
alone as proof.

### Cold Turbo Preview

Command:

```bash
scripts/gate-cold-turbo-preview.sh PHASE2-20260701-COLD-TURBO --systems arcade,neogeo,saturn --secs 10
```

Result:

- Arcade: `383/383` exact, miss `0`, blank `0`, stale `0`,
  first request-to-apply `129ms`, max request-to-apply `392ms`,
  `12` selected sync index-preads.
- NeoGeo: `388/388` exact, miss `0`, blank `0`, stale `0`,
  first request-to-apply `285ms`, max request-to-apply `25168ms`,
  `11` selected sync index-preads.
- Saturn: `382/382` exact, miss `0`, blank `0`, stale `0`,
  first request-to-apply `62ms`, max request-to-apply `21311ms`,
  `38` selected sync index-preads.

Read:

The zero-miss cold turbo preview goal holds on all three systems. The high
NeoGeo/Saturn max request-to-apply values did not surface as stale or blank
frames, but they deserve a closer look because they imply queue-age or
superseded-request accounting can report very high latency even when visible
correctness is perfect.

### Launch Handoff

Commands:

```bash
scripts/profile-launch-handoff.sh PHASE2-20260701-HANDOFF-SUCCESS --replace-label --iterations 3 --mode success
scripts/profile-launch-handoff.sh PHASE2-20260701-HANDOFF-SLOWFAIL --replace-label --iterations 3 --mode slow-fail --delay-ms 750
```

Rows appended to `history/toolchain-bench/results-launch-handoff.tsv`.

Success:

- Iteration 1 max loading-frame gap: `34263us`.
- Iteration 2 max loading-frame gap: `22387us`.
- Iteration 3 max loading-frame gap: `17583us`.
- Handoff wait: about `750ms`.

Slow-fail:

- Iteration 1 max loading-frame gap: `31603us`.
- Iteration 2 max loading-frame gap: `17737us`.
- Iteration 3 max loading-frame gap: `17596us`.
- Failure recovery: `2391-2864us`.

Read:

The previous 100ms-class handoff frame gaps were not reproduced. Handoff still
has a visible first-iteration gap around 31-34ms and should keep a
`max_frame_gap_us` gate, but this is now a polish/rare-spike issue rather than
the biggest performance problem.

### Launch Prep

Commands:

```bash
scripts/profile-launch-prep.sh PHASE2-20260701-LAUNCH-WARM --replace-label --scenario warm --iterations 5
scripts/profile-launch-prep.sh PHASE2-20260701-LAUNCH-COLD --replace-label --scenario cold --iterations 3
```

Rows appended to `history/toolchain-bench/results-launch-prep.tsv`.

Warm summary:

- Count: `60`.
- Errors: `0`.
- p50: `20us`.
- p95: `3190us`.
- Write bytes: `94208`.

Cold summary:

- Count: `36`.
- Errors: `0`.
- p50: `15us`.
- p95: `2754us`.
- Write bytes: `49152`.

Read:

Launch prep is cheap. Virtual NeoGeo structured plans are essentially free, and
AmigaVision descriptor writes are low-millisecond operations. This is not a
primary exFAT optimization target.

### First Scan

Command:

```bash
scripts/profile-first-scan.sh PHASE2-20260701-FIRSTSCAN --skip-build --replace-label --thread-sample
```

Rows appended to `history/toolchain-bench/results-first-scan.tsv`.

Key timings:

- First frame: `94ms`.
- Targets: `21`.
- First candidate: `2314ms`.
- First discovery: `2400ms`.
- Walk: `44881ms`, candidates `10637`.
- File discovery: `32739ms`, files `10617`.
- Classify total: `45337ms`, discoveries `13330`.
- Coverage audit: `5918ms`, rows `468`.
- Library scan complete: `52403ms`.
- RAM catalog ready: `56591ms`, games `11041`, catalog projection
  `4127703us`.
- SQLite import total: `10020ms`.
- SQLite publish: `708ms`, bytes `10268672`.
- Durable DB saved: `71789ms`, DB count `13205`.

Thread sample:

- Main/UI `mister-magik-fb` spent most samples on CPU1.
- `library-catalog` and `library-walker` spent samples on CPU0, often in
  D-state, consistent with storage waits.
- Media worker was mostly sleeping during first scan.

Read:

First scan is now the dominant production risk and is close to the documented
hard gate. The current larger library has made the previous 39s/47s class run
into a 56.6s/71.8s run. The biggest costs are exFAT metadata walk, file
discovery/classification, coverage audit, and RAM catalog projection. SQLite
publish is only about 0.7s.

### Library I/O And Save

Commands:

```bash
scripts/profile-library-io.sh PHASE2-20260701-LIBIO --replace-label --sample-limit 180
scripts/profile-library-save.sh PHASE2-20260701-LIBSAVE --iterations 5 --replace-label
```

Rows appended to:

- `history/toolchain-bench/results-library-io.tsv`
- `history/toolchain-bench/results-library-save.tsv`

Library I/O:

- Warm scan: about `5.3s`.
- Discover/walk: about `4.6s`.
- Classify: about `5.1s`.
- Import: about `13.6s`.
- Publish copy: `684ms`.

Library save publish iterations:

- Iteration 1: `718ms`.
- Iteration 2: `944ms`.
- Iteration 3: `1027ms`.
- Iteration 4: `917ms`.
- Iteration 5: `969ms`.
- DB size: `10268672` bytes.

Read:

When SD metadata is warm, scanning is no longer terrifying, but import and
projection/materialization still matter. The final file publish is sub-second
to about one second and should not be optimized before scan/import/projection
work.

### Screenshot Media

Commands:

```bash
scripts/profile-screenshot-save.sh PHASE2-20260701-SAVE-NEOGEO --system neogeo --iterations 10 --replace-label
scripts/profile-screenshot-download.sh PHASE2-20260701-DOWNLOAD --system neogeo --iterations 1 --replace-label
scripts/profile-media-cold-boot.sh PHASE2-20260701-MEDIA-COLD --skip-build --replace-label --timeout 900 --thread-sample
```

Rows appended to:

- `history/toolchain-bench/results-screenshot-save.tsv`
- `history/toolchain-bench/results-screenshot-download.tsv`
- `history/toolchain-bench/results-media-cold-boot.tsv`

NeoGeo save:

- Bytes: `4973975`.
- Iteration totals: `747ms`, `533ms`, then mostly `379-403ms`.

NeoGeo download:

- Total: `1056ms`.
- Wire: `331ms`.
- Verify: `288ms`.
- Save: `433ms`.
- Publish copy: `393ms`.
- Cloudflare cache: `HIT`.

Media cold boot with catalog reset:

- Arcade: first `3424ms`, done `58163ms`.
- Saturn: first `37174ms`, done `58171ms`.
- NeoGeo: first `58126ms`, done `59475ms`.
- All three systems: UI row seen, rendered, download progress seen, terminal
  `done`, validity `ok`.

Read:

For small cached packs, media publish is real but manageable. The combined cold
boot result shows media work overlaps the long catalog-first-scan window. Larger
Arcade/Saturn packs remain better candidates for stream-to-hidden-exFAT-temp
A/B testing than NeoGeo.

## Phase One Findings

### 1. Worker Bursts Can Still Land In Visible Frames

`LauncherScheduler::poll_catalog` and `poll_media` drain every available worker
message in one frame. Catalog progress is coalesced, and catalog-ready messages
can be deferred while Arcade is moving, but the frame loop still has no general
per-frame budget for background messages.

Relevant code:

- `magik-gui/src/ui_runner/launcher_scheduler.rs`
- `magik-gui/src/ui_runner/launcher_loop.rs`
- `magik-gui/src/ui_runner/launcher_frame_accounting.rs`

Why it matters:

The frame trace already has `catalog_worker_us`, `catalog_message_count`,
`catalog_backlog`, and `media_worker_us`, which is the right observability
surface. Prior evidence showed first-preview slow frames attributed almost
entirely to `catalog_worker_us`. On a dual-core Cortex-A9, the heavy work can be
off-thread and still cause a visible spike if the completion burst is processed
inside one UI frame.

Optimization direction:

- Budget catalog/media message processing per frame.
- Coalesce low-value timing/progress messages before they reach the UI loop.
- Keep `Ready` and launch-return hydration messages high priority, but defer
  projection repair, durable-save completion, media metadata, and non-visible
  progress when interaction is active.

Evidence needed:

- Preview scroll and first-preview traces with `catalog_worker_us`,
  `catalog_message_count`, and `catalog_backlog`.
- First-scan trace around `catalog_worker_ram_catalog` through
  `catalog_worker_saved_catalog`.

### 2. Launch Handoff Has Scheduling And Probe Blind Spots

The launch worker thread is spawned without an explicit runtime thread policy,
and the post-success runtime action probes shell out through `sh -c` with
`pidof` and `/proc/$pid/cmdline`.

Relevant code:

- `magik-gui/src/ui_runner/launch_handoff_session.rs`
- `magik-gui/src/launcher.rs`
- `magik-gui/catalog/src/runtime_thread.rs`

Why it matters:

The launch path is latency-sensitive: the loading frame is correctly presented
before handoff work starts, but handoff worker activity and runtime probing can
still collide with visible loading frames. Prior hardware evidence showed
approximately 100 ms max frame gaps during launch handoff iterations.

Optimization direction:

- Add a `LaunchHandoff` runtime thread role or at least emit inherited
  thread-policy rows for the worker.
- Replace shell-based arcade-core probing with a cheaper Main status/ack path,
  or throttle probes with an explicit next-probe timestamp.
- Add/keep a handoff gate on `max_frame_gap_us`, not only final success or
  failure.

Evidence needed:

- `profile-launch-handoff` success and slow-fail runs with thread sampling.
- A process/thread sample covering `mister-magik-fb`, `launch-handoff`,
  `MiSTer_MagiK`, and any shell/helper processes.

### 3. Rendering Is Healthy But Attribution Is Incomplete

The production renderer is well-shaped: Slint renders to cached RAM, direct
Arcade/preview layers avoid unnecessary Slint work, and RGB565 is the only
production contract. The main attribution gap is direct preview present.

Relevant code:

- `magik-gui/src/ui_runner/launcher_compositor.rs`
- `magik-gui/src/ui_runner/ui_frame_target.rs`
- `magik-gui/src/ui_runner/raw565_preview_renderer.rs`
- `magik-gui/src/arcade_list_renderer.rs`

Why it matters:

`LauncherCompositor::present` times cached presents and Arcade list presents,
but direct preview present is included only in the larger framebuffer-present
window. The production direct preview path is now default, so future changes
need a separate `direct_preview_present_us` and ideally copied-row/byte counts.

The Arcade list renderer already avoids rerendering the whole list by scrolling
a RAM surface and redrawing exposed bands, but presentation still copies the
list layer. This remains one of the larger steady-state memory-bandwidth costs,
although current evidence says it is comfortably within budget.

Optimization direction:

- Add `direct_preview_present_us` and `direct_preview_rows`.
- Add fast-path counters for preview fade geometry:
  same-geometry, single-geometry, and fallback.
- Test narrower Arcade copy segmentation only with before/after present traces.
  The live-framebuffer scroll-present path remains a likely dead end based on
  prior measurements.

Evidence needed:

- `profile-preview-scroll` and `gate-preview-60fps`.
- `launcher-present-trace.py compare` for copy/render changes.

### 4. First Scan Is Mostly exFAT Walking, Not SQLite

Current evidence and code shape point to recursive exFAT directory walking as
the dominant first-scan cost. SQLite build/publish is already designed well:
build in tmpfs, use fast SQLite pragmas, then copy/sync/rename to `/media/fat`.

Relevant code:

- `magik-gui/catalog/src/catalog_scan.rs`
- `magik-gui/catalog/src/library_indexer.rs`
- `magik-gui/catalog/src/sqlite_catalog.rs`
- `magik-gui/catalog/src/catalog_checkpoint.rs`
- `magik-gui/catalog/src/core_audit.rs`

Why it matters:

The SD card/exFAT path punishes metadata-heavy recursive scans. Warm validation
is currently cheap because the root stamp/checkpoint avoids full enumeration,
but it can become growth-sensitive when unknown top-level dirs or diagnostic
payload checks force additional metadata reads.

Optimization direction:

- Reduce metadata walks before spending more effort on SQLite tuning.
- Consider a production `.magik-catalog-dirindex` sidecar written after a
  successful scan and used only as a rebuild hint, never as authoritative launch
  data.
- Order scan targets by high-yield/low-depth systems so the RAM catalog can
  become partially useful earlier while final SQLite still owns canonical
  ordering.
- Cap or cache `game_dir_has_payloadish_files` for unknown dirs in warm drift
  detection.
- Avoid extra post-publish SQLite reloads for projections if benchmarks show
  projection write cost matters.

Evidence needed:

- `profile-first-scan` with `--thread-sample`.
- `profile-library-io`.
- `profile-library-save`.
- `scan_stage_walk_target`, `library_ready`, `library_db_saved`,
  `import_stage_total`, and `sqlite_publish_progress`.

### 5. Media Pack Publish Is The Biggest Remaining exFAT Write Win

The media worker downloads to `/tmp`, verifies, then publishes by copying to a
hidden/temp path on `/media/fat` and syncing. This is safe, but it means large
packs pay a full extra exFAT copy.

Relevant code:

- `magik-gui/src/ui_runner/media_worker.rs`
- `magik-gui/src/media_pack_save.rs`
- `magik-gui/src/artifact_publish.rs`

Why it matters:

Pack saves have historically taken hundreds of milliseconds to several seconds,
depending on system size. This is acceptable for visible media updates, but it
is the clearest I/O optimization target that does not disturb the renderer.

Optimization direction:

- Promote the existing stream-to-hidden-exFAT-temp while hashing strategy from
  benchmark-only to a production A/B.
- Keep checksum verification before rename, hidden temp paths, and cleanup on
  failure.
- Keep production media concurrency at one.
- Defer starting new downloads while interaction is active; consider pausing or
  delaying publish phases if user interaction starts after a download began.

Evidence needed:

- `profile-screenshot-save`.
- `profile-screenshot-download`.
- `profile-media-cold-boot` with thread sampling.
- Process samples for `curl`/hash helper scheduling.

### 6. Preview Index Fast Lane Is Good, But Cold Random I/O Remains Important

The `.idx` sidecar path avoids full archive preload and uses `pread`, which is
the right design for cold first selected previews. The remaining risks are
repeated open/close, cold random reads, metadata TTL checks, and shared decoded
cache contention between selected and prefetch workers.

Relevant code:

- `magik-gui/catalog/src/preview_worker.rs`
- `magik-gui/src/preview_state.rs`
- `magik-gui/src/ui_runner/raw565_preview_renderer.rs`

Optimization direction:

- Cache an open archive `File` per sidecar index if cold turbo preview evidence
  shows open/close cost.
- Make prefetch skip or back off on decoded-cache mutex contention if selected
  preview queue age spikes.
- Keep full archive memory promotion for idle windows and frequently used
  systems, not as the first response to every selected preview.

Evidence needed:

- `profile-first-preview`.
- `profile-preview-scroll --skip-preview-warm`.
- `gate-cold-turbo-preview --systems arcade,neogeo,saturn`.
- Selected preview request age, read/decode/total, load source, and prefetch
  activity.

## Phase Two Hardware Matrix

Start with a known production bench-tools binary:

```bash
scripts/mister status
scripts/mister db "SELECT count(*) FROM games"
scripts/deploy-rust.sh --device --ui-scope launcher --bench-tools
```

Primary production runs:

```bash
scripts/profile-warm-catalog-start.sh PHASE2-20260701-WARM --replace-label --iterations 5
scripts/profile-arcade-scroll.sh PHASE2-20260701-ARCADE --secs 30 --scenario turbo-hold --skip-build --thread-sample
scripts/profile-preview-scroll.sh PHASE2-20260701-PREVIEW --secs 30 --scenario turbo-hold --skip-build --visual-captures 0 --thread-sample
scripts/profile-first-preview.sh PHASE2-20260701-FIRST-PREVIEW --skip-build
scripts/profile-preview-scroll.sh PHASE2-20260701-PREVIEW-COLD --secs 30 --scenario turbo-hold --skip-build --skip-preview-warm --visual-captures 0 --thread-sample
scripts/gate-preview-60fps.sh PHASE2-20260701-GATE --skip-build --visual-captures 0
scripts/profile-first-scan.sh PHASE2-20260701-FIRSTSCAN --skip-build --replace-label --thread-sample
scripts/profile-library-io.sh PHASE2-20260701-LIBIO --replace-label --sample-limit 180
scripts/profile-library-save.sh PHASE2-20260701-LIBSAVE --iterations 5 --replace-label
scripts/profile-screenshot-save.sh PHASE2-20260701-SAVE-NEOGEO --system neogeo --iterations 10 --replace-label
scripts/profile-screenshot-download.sh PHASE2-20260701-DOWNLOAD --system neogeo --iterations 1 --replace-label
scripts/profile-media-cold-boot.sh PHASE2-20260701-MEDIA-COLD --skip-build --replace-label --timeout 900 --thread-sample
scripts/profile-launch-handoff.sh PHASE2-20260701-HANDOFF-SUCCESS --replace-label --iterations 3 --mode success
scripts/profile-launch-handoff.sh PHASE2-20260701-HANDOFF-SLOWFAIL --replace-label --iterations 3 --mode slow-fail --delay-ms 750
scripts/profile-launch-prep.sh PHASE2-20260701-LAUNCH-WARM --replace-label --scenario warm --iterations 5
scripts/profile-launch-prep.sh PHASE2-20260701-LAUNCH-COLD --replace-label --scenario cold --iterations 3
```

CPU attribution, if needed after the production traces:

```bash
scripts/profile-preview-scroll.sh PHASE2-20260701-PREVIEW-CPU --secs 30 --scenario turbo-hold --cpu-profile --visual-captures 0
scripts/deploy-rust.sh --device --ui-scope launcher --bench-tools
```

The second deploy restores the production bench-tools binary after the profiling
binary.

Conditional runs:

```bash
scripts/device-startup-reveal-acceptance.sh PHASE2-20260701-REVEAL
scripts/gate-cold-turbo-preview.sh PHASE2-20260701-COLD-TURBO --systems arcade,neogeo,saturn --secs 10
```

## Priority Optimization Backlog

1. Treat first scan as the main performance problem:
   reduce exFAT metadata walks, coverage-audit cost, and RAM catalog projection
   time before chasing sub-second SQLite publish wins.
2. Add missing observability before changing hot paths:
   `direct_preview_present_us`, `direct_preview_rows`, preview fade fast-path
   counters, launch-handoff thread-policy rows, and UI-thread startup
   nice/CPU logging.
3. Prove and then cap worker message processing per visible frame. The preview
   traces show rare work misses that are not steady-state copy/render problems.
4. Tighten preview scheduling and selected/prefetch accounting. Cold
   index-pread correctness is excellent, but rare scheduler spikes and high
   max request-to-apply values remain.
5. Replace or throttle shell-based launch runtime probing, and keep
   `max_frame_gap_us` in the launch handoff gate.
6. A/B stream-to-hidden-exFAT-temp media publish on larger Arcade/Saturn packs.
   NeoGeo is too small and cache-friendly to be a decisive test.
7. A/B open archive file caching for preview index `pread`.
8. Only after those, revisit steady Arcade list copy segmentation.

## Likely Dead Ends

- Direct Slint rendering into live framebuffer memory.
- Wider-color launcher modes or RGB888/ARGB framebuffer A/B paths.
- Runtime PNG/JPEG decode or preview cache rebuilds on the MiSTer hot path.
- Full archive mmap/preload as default for large packs on a 1 GiB device.
- Removing sync/fsync without a replacement crash-safety contract.
- `gamelist.xml` walks or scanning screenshot/cache media directories.
- Row-by-row selected-index benchmarks for Arcade conclusions.
- Experimental effects, mega transitions, and dense present experiments as
  production evidence.
