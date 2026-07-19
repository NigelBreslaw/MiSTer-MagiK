# Performance Review - 2026-07-05

Scope: production MiSTer MagiK code only. Experimental effects and effect-scene
benchmarks are intentionally excluded. Phase 1 is code review only; Phase 2
will add real-hardware measurements from the reference MiSTer.

## Executive Summary

The project already has the right high-level performance shape for the
hardware: RGB565-only rendering, cached RAM render targets, dirty-region
framebuffer copies, Rust-painted Arcade/preview direct layers, a selected
preview worker separate from lower-priority prefetch, off-/tmp SQLite building,
and documented production benchmark gates.

The largest code-level opportunities are now more specific:

1. Reduce avoidable hot-loop work in frames where only direct overlays move.
2. Make framebuffer copy decisions byte/area aware instead of width-threshold
   only.
3. Protect the UI/vsync path explicitly on the dual-core Cortex-A9 while
   keeping background work pinned/niced.
4. Remove remaining synchronous filesystem and process-spawn work from the UI
   loop.
5. Collapse duplicated warm-validation directory walks and tiny exFAT reads.
6. Add benchmark evidence that reports core residency, SD-card I/O, and exact
   present bytes, not only FPS.

## Phase 1 Findings - Code Review Only

### 1. Overlay-only Arcade frames still pay Slint frame overhead

Files:

- `apps/mister/src/ui_runner/launcher_loop.rs`
- `apps/mister/src/ui_runner/launcher_compositor.rs`
- `apps/mister/src/ui_runner/ui_frame_target.rs`

The launcher has a strong direct-layer design: Arcade list and raw previews can
be rendered outside Slint and copied as direct RGB565 overlays. Even so, the
main loop still updates Slint animation state and calls `draw_if_needed` before
compositing. During pure Arcade scroll or preview-only movement, this leaves
CPU on the table.

Optimization:

- Add a conservative overlay-only fast path when all of these are false:
  bridge dirty, Slint animation active, modal/loading/search UI active, route
  recovery, catalog/media messages that need visible UI, composition recovery,
  and any cached-base damage.
- In that path, skip Slint base render and present only direct preview/list
  layers.
- Measure with `profile-preview-scroll.sh` and `profile-arcade-scroll.sh`,
  comparing `slint_render_us`, `arcade_list_present_us`,
  `direct_preview_present_us`, `p99_work_us`, and visual capture parity.

Risk: medium. The guard must be strict enough not to freeze clocks, popups,
modal state, startup overlays, or bridge-driven UI.

### 2. Broad dirty rects may copy too many framebuffer bytes

Files:

- `mister/platform/runtime/src/framebuffer/target.rs`
- `apps/mister/src/ui_runner/launcher_loop.rs`

`copy_cached_rect_565` promotes any rect that is full width or at least 85% of
the framebuffer width into a full-row copy. This is often good for contiguous
write-combined `/dev/fb0` writes, but some wide-but-not-full bands, such as Home
pan damage, cross the threshold and copy extra pixels every frame.

Optimization:

- Make the broad-rect policy byte/area aware.
- Add trace columns for `present_pixels`, `present_bytes`, and
  `wasted_present_bytes` per layer.
- Benchmark 85%, 95%, and exact-strided policies on real hardware before
  changing the default.

Risk: low to medium. Contiguous full-row writes can beat narrower strided
writes despite extra bytes, so this must be measured on `/dev/fb0`.

### 3. UI and vsync threads are not explicitly protected

Files:

- `crates/catalog/src/runtime_thread.rs`
- `mister/platform/runtime/src/framebuffer/vsync.rs`
- `crates/catalog/src/preview_worker.rs`
- `apps/mister/src/ui_runner/media_worker.rs`

Background catalog/media/prefetch work is pinned or niced, and foreground first
scan intentionally owns both cores. The UI frame loop and vsync worker,
however, inherit default scheduler placement. Selected preview and media
download work run at normal priority on any CPU, so they can contend with the
UI path.

Optimization:

- Add runtime roles for `UiFrameLoop` and `Vsync`.
- Test pinning UI+vsync to CPU1 while background workers stay on CPU0.
- Keep selected preview at normal priority but test whether it should avoid the
  UI core during active scroll.
- Promote thread sampling from optional evidence to a normal release perf gate.

Risk: medium. Over-pinning can reduce scheduler flexibility, especially during
first scan and visible media download. Treat this as a benchmarked policy,
not a static assumption.

### 4. Launch handoff probes spawn a shell from the UI loop

Files:

- `apps/mister/src/ui_runner/launcher_loop.rs`
- `apps/mister/src/ui_runner/launch_handoff_session.rs`
- `apps/mister/src/launcher.rs`

After handoff, the launcher can call a core-running probe that shells out to
`pidof` and reads `/proc/.../cmdline`. During a sensitive transition, repeated
process spawns can create CPU spikes and delay recovery detection.

Optimization:

- Check elapsed debounce gates before probing.
- Throttle shell/process probes to a fixed interval.
- Prefer Main status/ack state or an already-open proc/status path over
  spawning `sh`.

Risk: low to medium. Launch recovery is correctness-sensitive, so keep the
existing behavior as fallback until the Main-side signal is trusted.

### 5. Warm startup can synchronously load full SQLite before first frame

Files:

- `apps/mister/src/ui_runner/launcher_loop.rs`
- `crates/catalog/src/sqlite_catalog.rs`
- `crates/catalog/src/catalog_summary.rs`
- `crates/catalog/src/catalog_navigation.rs`

Warm boot is optimized around `library.summary.json` and `library.nav.lz4b`.
If those are missing or stale, the UI path can fall back to full SQLite catalog
load before the first intended frame. That is exactly the kind of cold exFAT
small-read path that can make HDMI stay black longer.

Optimization:

- Make pre-frame startup depend only on summary/navigation projections.
- If projections are absent, show a bounded loading state and move full SQLite
  hydration to the catalog worker.
- Add a timing gate for "sync work before first requested redraw".

Risk: medium. Arcade direct-start and return-from-game need hydrated rows, so
their reveal rules must remain explicit and bounded.

### 6. Worker result bursts are processed without a per-frame budget

Files:

- `apps/mister/src/ui_runner/launcher_scheduler.rs`
- `apps/mister/src/ui_runner/launcher_loop.rs`
- `apps/mister/src/preview_state.rs`

Catalog, media, and preview channels are drained eagerly. The off-thread design
is good, but result bursts can still collapse into one expensive UI frame,
especially when catalog `Ready` triggers bridge/model sync.

Optimization:

- Add per-frame message or microsecond budgets for non-critical worker results.
- Coalesce timing/progress rows before UI application.
- Split "catalog ready" into cheap state adoption and separately budgeted
  bridge/model sync where possible.

Risk: medium. Startup reveal and selected-preview exactness have deadlines, so
budgeting must prioritize visible correctness events over diagnostics.

### 7. Warm validation repeats filesystem discovery work

Files:

- `crates/catalog/src/sqlite_catalog.rs`
- `crates/catalog/src/core_audit.rs`
- `crates/catalog/src/catalog_checkpoint.rs`
- `crates/catalog/src/catalog_discovery.rs`

Warm stamp validation computes audit rows, stamp, and discovery checkpoint from
overlapping directory and metadata facts. On exFAT/FUSE, duplicated `read_dir`,
`metadata`, and shallow payload checks are costly enough to threaten the
sub-500ms soft target.

Optimization:

- Build one `WarmValidationSnapshot` containing root signatures, installed
  cores, top-level game-dir summaries, metadata DB signatures, and audit facts.
- Derive stamp, checkpoint, and drift detail from that snapshot.
- Prefer `DirEntry::file_type()` over fresh metadata calls where it is
  reliable.

Risk: medium. Drift semantics are correctness-sensitive; tests should assert
the exact same unchanged/changed outcomes before and after.

### 8. Cold scan is pipelined, but expensive classification is serial

Files:

- `crates/catalog/src/catalog_scan.rs`
- `crates/catalog/src/library_indexer.rs`
- `crates/catalog/src/media_metadata.rs`

Cold scan uses a walker thread feeding a classifier over a bounded channel. That
is good. However, expensive work such as ZIP central-directory parsing, MRA/MGL
metadata reads, and 7za-backed collection listings can still serialize behind
one classifier.

Optimization:

- Keep deterministic walking order.
- Add a tiny bounded worker pool only for expensive archive/metadata jobs.
- Merge results in stable order before projection/import.
- Batch collection listing extraction per archive instead of spawning `7za` per
  listing path.

Risk: medium. Progress events, ordering, and first-scan gates need careful
regression coverage.

### 9. Preview pack path resolution can do repeated state-file work

Files:

- `crates/catalog/src/preview_worker.rs`
- `crates/catalog/src/sqlite_catalog.rs`
- `apps/mister/src/ui_runner/media_worker.rs`

Preview requests and SQLite hydration resolve active screenshot pack paths from
media state and filesystem checks. This is not huge, but it happens on paths
where first-preview latency and scroll smoothness matter.

Optimization:

- Cache `system_id -> active archive path` with a media-state fingerprint and
  short TTL.
- Invalidate this cache together with existing preview archive metadata
  invalidation when media updates publish new state.

Risk: low. The main risk is stale pack selection after a media update.

### 10. First-preview fast path is good, but full-pack warm can contend

Files:

- `crates/catalog/src/preview_worker.rs`
- `docs/benchmarking.md`

The `.mmlz4b.idx` sidecar `pread` path is the right cold first-preview design.
The optional background full-pack warm can still compete with selected preview
reads for SD-card bandwidth, especially because the Arcade pack is tens of MB
and takes seconds to cold-read.

Optimization:

- Delay full archive warming until input settles.
- Measure scroll while warm is active and report selected-preview age/load
  source.
- Test single-file-handle full warm after index lookup to avoid reopen/stats.
- Treat `mmap` as an experiment only if prefaulting is explicit; lazy page
  faults during scroll are likely worse.

Risk: medium. Full-pack warm helps steady-state; delaying it may reduce cache
readiness if the user immediately scrolls far.

### 11. Raw565 decode still does a byte-to-u16 copy pass

Files:

- `crates/catalog/src/preview_worker.rs`
- `apps/mister/src/raw565.rs`
- `apps/mister/src/ui_runner/raw565_preview_renderer.rs`

Raw565 preview payloads are parsed into `Vec<u16>`. For a 320x320 preview, that
is roughly 200 KB of memory traffic per decode after the read/decompress step.

Optimization:

- For aligned little-endian raw565 payloads, decode directly into the final
  RGB565 word buffer.
- Keep the current parser as the portable/fallback path.
- Add decode metrics that separate read, parse/copy, and cache-insert cost.

Risk: medium. Endianness, stride, and alignment correctness need tests and real
visual captures.

### 12. Arcade filter/search rows allocate and convert in hot paths

Files:

- `apps/mister/src/arcade_list_renderer.rs`

Game rows are cached, but filter rows are rendered as `Vec<Pixel>` and then
converted to RGB565. Filter/search navigation can allocate and touch more memory
than necessary.

Optimization:

- Render row text directly into `Rgb565Pixel` buffers.
- Cache filter rows by title/count/active/parity.
- Reuse scratch buffers and precomputed RGB565 backgrounds.

Risk: low to medium. Needs visual parity checks.

### 13. Selected-row inversion does an extra pass

Files:

- `apps/mister/src/arcade_list_renderer.rs`

The selected aperture is copied through an inversion scratch buffer before
presentation. This avoids mutating the cached layer, but costs an extra pass.

Optimization:

- Test a direct inverted RGB565 present path.
- Or cache the current pre-inverted selected strip and invalidate only when
  selection or row pixels change.

Risk: medium. The selected-row visual is subtle; benchmark with visual captures.

### 14. SQLite/projection publish has multiple sync tails

Files:

- `crates/catalog/src/sqlite_catalog.rs`
- `crates/catalog/src/catalog_summary.rs`
- `crates/catalog/src/catalog_navigation.rs`

The database itself is built off `/media/fat` and published safely, which is
good. The DB, summary, and nav projection still create multiple sync/rename/
parent-sync tails on exFAT.

Optimization:

- Batch summary and nav publish: write both temps, sync both files, rename
  both, then parent-sync once.
- Consider folding summary fields into the nav projection if it removes an
  artifact without hurting warm-start size.

Risk: medium. Crash consistency and stale-projection invalidation must remain
strict.

### 15. Benchmark reset misses one projection artifact

Files:

- `scripts/profile-first-scan.sh`
- `docs/catalog.md`

`profile-first-scan.sh` deletes the SQLite database and summary projection, but
not the navigation projection. The app appears DB-gated today, so this likely
does not currently mask first scan, but the reset state is not a fully clean
"no catalog" state.

Optimization:

- Delete `library.nav.lz4b` in first-scan setup.
- Add an artifact-reset row to the benchmark output listing which catalog
  artifacts were removed.

Risk: low. This is mostly benchmark hygiene.

## Phase 2 Benchmark Plan

Production-only run order:

1. `scripts/mister status`
2. `scripts/bench-toolchain.sh PERF-20260705-HOME --replace-label --device --scene-secs 30 --launcher-scenario home-repeat-hold --ui-scope launcher`
3. `scripts/profile-arcade-scroll.sh PERF-20260705-ARCADE --secs 30 --scenario turbo-hold --skip-build --thread-sample`
4. `scripts/profile-preview-scroll.sh PERF-20260705-PREVIEW --secs 30 --scenario turbo-hold --skip-build --visual-captures 0 --thread-sample --replace-label`
5. `scripts/profile-first-preview.sh PERF-20260705-FIRSTPREVIEW --skip-build`
6. `scripts/gate-preview-60fps.sh PERF-20260705-GATE --skip-build --visual-captures 0`
7. `scripts/profile-warm-catalog-start.sh PERF-20260705-WARM --iterations 5 --replace-label`
8. `scripts/profile-launch-prep.sh PERF-20260705-LAUNCHPREP --scenario warm --iterations 5`
9. `scripts/profile-launch-handoff.sh PERF-20260705-HANDOFF --iterations 5 --mode slow-fail`
10. `scripts/profile-library-save.sh PERF-20260705-SAVE --iterations 5 --replace-label`
11. `scripts/profile-library-io.sh PERF-20260705-LIBIO --replace-label`
12. `scripts/profile-preview-index-refresh.sh PERF-20260705-IDX`
13. `scripts/profile-preview-pack-decode.sh PERF-20260705-PACK --iterations 5 --order random`
14. `scripts/gate-cold-turbo-preview.sh PERF-20260705-COLDTURBO --systems arcade,neogeo,saturn --secs 10`
15. `scripts/device-startup-reveal-acceptance.sh PERF-20260705-REVEAL`
16. `scripts/profile-first-scan.sh PERF-20260705-FIRSTSCAN --skip-build --replace-label --thread-sample`

Run notes:

- No `direct-reset-no-sync`.
- Rebooting/destructive catalog-state benchmarks run late.
- Effect-scene scripts under `scripts/experiments/` are excluded.
- CPU profiling may deploy a profiling binary. If used, restore production
  release-device afterward before interpreting production frame timings.

## Phase 2 Results

Measurements were run on the reference MiSTer at `192.168.1.117` using the
deployed `release-device` launcher-scope bench-tools binary:

- git commit: `8ec3a83f` with local benchmark/report artifacts dirty.
- binary: `/media/fat/mister-magik/mister-magik-fb`.
- local build artifact:
  `apps/mister/target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb`.
- binary size: `6,692,796` bytes.
- display path: RGB565 `960x540`, FPGA-scaled to HDMI.
- no experimental effect-scene benchmarks were run.

### Device State After Runs

Final status was healthy:

- `MiSTer_MagiK` running.
- `mister-magik-fb` running.
- active VT: `tty2`.
- framebuffer mode: `565 1 960 540 1920`.
- launcher screen: Home.

### Home Baseline

Command:

```bash
scripts/bench-toolchain.sh PERF-20260705-HOME --replace-label --skip-build --device --scene-secs 30 --launcher-scenario home-repeat-hold --ui-scope launcher
```

Result row in `history/toolchain-bench/results.tsv`:

- FPS: `55`.
- CPU mean/max: `100% / 136%`.
- `prepare_us`: `121`.
- `render_us`: `9017`.
- `copy_us`: `1309`.
- `cached_present_us`: `1290`.
- rows average: `403`.
- visual/timing/capture: pass.

Interpretation: Home is still Slint-render dominated. This is the clearest case
for reducing base-render frequency or damage area when only small Home elements
move.

### Arcade Scroll

Command:

```bash
scripts/profile-arcade-scroll.sh PERF-20260705-ARCADE --secs 30 --scenario turbo-hold --skip-build --thread-sample
```

Artifacts:

- `build/arcade-scroll-profiles/PERF-20260705-ARCADE-arcade-scroll.tsv`
- `build/arcade-scroll-profiles/PERF-20260705-ARCADE-arcade-scroll.log`
- `build/arcade-scroll-profiles/PERF-20260705-ARCADE-arcade-scroll-thread-sample.tsv`

Steady scroll rows after startup:

- frames: `839` to `857`, depending on summarizer window.
- vsync/fallback/timeout/error: `839/0/0/0`.
- max vsync miss streak: `0`.
- `wall_us` p50/p95/p99: `16632 / 16748 / 26789`.
- `prepare_us` p50/p95/p99: `1423 / 3562 / 13590`.
- `slint_render_us` p50/p95/p99: `253 / 322 / 390`.
- `custom_draw_us` p50/p95/p99: `1425 / 1747 / 8878`.
- `fb_present_us` p50/p95/p99: `1227 / 1495 / 1581`.
- direct preview present p50/p95/p99: `327 / 421 / 462`.
- arcade list present p50/p95/p99: `886 / 1065 / 1122`.
- rows: stable `800`.

Interpretation: the steady Arcade path is mostly good and is not Slint-render
bound. The remaining misses are bursty `prepare_us` and preview/list custom draw
work, not framebuffer copy alone.

### Preview Scroll

Command:

```bash
scripts/profile-preview-scroll.sh PERF-20260705-PREVIEW --secs 30 --scenario turbo-hold --skip-build --visual-captures 0 --thread-sample --replace-label
```

Artifacts:

- `build/preview-scroll-profiles/PERF-20260705-PREVIEW-arcade.tsv`
- `build/preview-scroll-profiles/PERF-20260705-PREVIEW-arcade.log`
- `build/preview-scroll-profiles/PERF-20260705-PREVIEW-arcade-report.html`
- `build/preview-scroll-profiles/PERF-20260705-PREVIEW-arcade-thread-sample.tsv`

Key rows:

- frames: `870`.
- steady frames after frame 30: `839`.
- exact preview frames: `868`.
- warm archive loaded in `25,785 us`.
- decoded preview rows: `722`.
- index `pread` loads: `404`.
- unexpected file reads: `0`.
- slow reads: `3`.
- prefetch queue age p95/p99/max: `634,028 / 999,458 / 1,402,328 us`.
- steady work p95/p99: `6,351 / 14,948 us`.
- steady work over 16.667 ms: `8` frames.
- slow-work attribution: `6` preview frames, `2` dominant-prepare frames.
- vsync/fallback/timeout/error: `839/0/0/0`.

Interpretation: preview correctness is strong, but turbo scroll can still spend
too much of one frame in selected-preview scheduling/apply/blit work. This
supports adding a per-frame budget and coalescing for preview result handling.

### 60 FPS Preview Gate

Command:

```bash
scripts/gate-preview-60fps.sh PERF-20260705-GATE --secs 30 --skip-build --visual-captures 0
```

Held-scroll fade:

- frames after 30: `809`.
- p99 work: `5,988 us` against the `14,500 us` threshold.
- work over 16.667 ms: `0`.
- vsync/fallback/timeout/error: `809/0/0/0`.
- max miss streak: `0`.

Turbo-hold fade:

- frames after 30: `800`.
- p99 work: `13,459 us` against the `14,500 us` threshold.
- work over 16.667 ms: `0`.
- vsync/fallback/timeout/error: `800/0/0/0`.
- max miss streak: `0`.

Interpretation: production preview scrolling passes the release gate, but turbo
is close enough to the p99 limit that small regressions in preview blit,
scheduler behavior, or SD-card warmth will be visible.

### First Preview / Cold Preview

Commands:

```bash
scripts/profile-first-preview.sh PERF-20260705-FIRSTPREVIEW --skip-build --replace-label
scripts/profile-preview-scroll.sh PERF-20260705-COLDPREVIEW --secs 12 --scenario held-scroll --skip-build --skip-preview-warm --visual-captures 0 --replace-label
```

Both runs failed their present-path validators because the trace had too few
steady frames. The logs are still useful:

- `PERF-20260705-FIRSTPREVIEW` produced an empty trace because startup entered
  `acornatom` with no preview candidate.
- first frame in that run was `83 ms`.
- `catalog_navigation_load` then took `2,402,219 us`, with `open_us=1,781,007`.
- `PERF-20260705-COLDPREVIEW` decoded `85` previews via index `pread`.
- cold decode total avg/max: `4,676 / 50,520 us`.
- cold read avg/max: `164 / 849 us`.
- cold decode+parse avg/max: `4,208 / 50,268 us`.

Interpretation: the first-preview helper needs a deterministic preview-bearing
system/row. More importantly, navigation projection hydration can dominate the
early window and should be gated or deferred separately from preview timing.

### Warm Catalog Startup

Command:

```bash
scripts/profile-warm-catalog-start.sh PERF-20260705-WARM --iterations 5 --replace-label
```

Rows in `history/toolchain-bench/results-warm-catalog.tsv`:

- first frame average: `66.8 ms`.
- first frame range: `62..75 ms`.
- summary load average: `13,770 us`.
- bridge systems/sync: roughly `139..207 us` / `73..78 us`.
- full catalog ready average: `1,435 ms`.
- navigation load average: `1,353,067 us`.

Interpretation: warm reveal is good. The remaining warm-start cost is the
post-reveal navigation/full-catalog tail, which can interfere with early
benchmarks and possibly with early user input.

### Launch Prep

Command:

```bash
scripts/profile-launch-prep.sh PERF-20260705-LAUNCHPREP --scenario warm --iterations 5 --replace-label
```

Summary row:

- samples: `60`.
- errors: `0`.
- p50: `15 us`.
- p95: `9,334 us`.
- read bytes: `512`.
- write bytes: `98,304`.
- descriptor writes: `20`.
- descriptor bytes: `540`.

Interpretation: structured virtual Neo Geo launch refs are essentially free.
AmigaVision descriptor updates are the tail: tiny descriptor payloads still
become 4 KiB exFAT writes and several milliseconds of latency. Avoiding
unchanged descriptor writes is the obvious win here.

### Launch Handoff

Command:

```bash
scripts/profile-launch-handoff.sh PERF-20260705-HANDOFF --iterations 5 --mode slow-fail --replace-label
```

The script emitted only two samples despite `--iterations 5`; treat that as a
benchmark-script gap.

Captured samples:

- loading visible after launch action: `46,173 us` and `49,949 us`.
- max frame gap: `17,238 us` and `17,531 us`.
- loading frames before result: `47`.
- simulated failure recovery: `2,398 us` and `2,387 us`.
- handoff wait: about `750 ms`.

Interpretation: the loading/recovery UI path is responsive. The code-review
finding about shell/proc handoff probes remains worth fixing, but the simulated
slow-fail path did not show a large UI stall here.

### Library Save

Command:

```bash
scripts/profile-library-save.sh PERF-20260705-SAVE --iterations 1 --replace-label
```

Result:

- scanned/discovered entries: `61,626`.
- normal files: `44,543`.
- containers: `2,814`.
- archive/container entries: `16,834`.
- `scan_us`: `28,879,004`.
- `discover_us`: `19,609,592`.
- `classify_us`: `25,940,738`.
- `import_us`: `49,454,743`.
- SQLite bytes published to exFAT: `41,885,696`.
- publish copy: `2,911 ms`.
- publish total: `2,941 ms`.
- progress events: `161`.

Interpretation: final exFAT publication is material, but the bigger target is
scan/classify/import. The DB is now much larger than prior 10 MB benchmark rows,
so first-scan gates need to track library size or row count explicitly.

### Library I/O

Command:

```bash
scripts/profile-library-io.sh PERF-20260705-LIBIO --replace-label --sample-limit 120
```

Result rows in `history/toolchain-bench/results-library-io.tsv`:

- total elapsed: `88 s`.
- `scan_us`: `34,327,124`.
- `discover_us`: `24,604,091`.
- `classify_us`: `31,243,791`.
- `import_us`: `51,333,755`.
- import-stage total: `24,261 ms`.
- insert games total: `10,108 ms`.
- materialize Arcade UI: `4,753 ms`.
- insert launcher Arcade: `2,030 ms`.
- insert launcher console: `2,311 ms`.
- insert catalog stamp: `1,113 ms`.
- SQLite publish: `41,885,696` bytes, `3,504 ms` total.

I/O samples show the first phase accumulating over `114 MB` of process reads,
then the publish phase adding about `45 MB` of process writes near the end.

Interpretation: this is dual-core CPU plus SD-card metadata pressure, not just
a final write bottleneck. The fastest path is to reduce repeated filesystem
metadata work and expensive classification, then keep SQLite/projection publish
compact.

### Cold Turbo Preview Gate

Command:

```bash
scripts/gate-cold-turbo-preview.sh PERF-20260705-COLDTURBO --systems arcade,neogeo,saturn --secs 10
```

Results:

- Arcade: pass. `272` previewable selections, `272` exact, `0` misses,
  `48` index `pread` loads, no archive-memory loads.
- Saturn: pass. `248` previewable selections, `248` exact, `0` misses,
  `10` index `pread` loads, no archive-memory loads.
- Neo Geo: fail due to benchmark coverage. The scripted window found `0`
  previewable selections and no preview requests, so it did not prove preview
  failure.

Interpretation: the index `pread` first-preview lane works well for Arcade and
Saturn under cold direct-to-system turbo. The Neo Geo fixture needs either a
different starting row or pack/key coverage validation.

### First Scan

Command:

```bash
scripts/profile-first-scan.sh PERF-20260705-FIRSTSCAN --skip-build --replace-label --thread-sample --timeout 240
```

Artifacts:

- `build/first-scan-profiles/PERF-20260705-FIRSTSCAN-slint.log`
- `build/first-scan-profiles/PERF-20260705-FIRSTSCAN-first-scan-thread-sample.tsv`

Result: failed both gates.

- first frame: `489 ms`.
- library scan complete: `92,781 ms`.
- library ready: `101,798 ms` against a `57,094 ms` gate.
- DB saved: `155,465 ms` against a `72,573 ms` gate.
- scan_us in final row: `104,563,468`.
- discover_us: `42,486,275`.
- classify_us: `43,275,025`.
- import_us: `39,993,422`.
- insert games total: `14,353 ms`.
- materialize Arcade UI: `6,432 ms`.
- insert launcher Arcade: `2,615 ms`.
- insert launcher console: `3,349 ms`.
- insert catalog stamp: `1,468 ms`.
- SQLite import total: `33,241 ms`.
- final publish: `41,885,696` bytes, `2,966 ms`.

Thread policy rows show:

- preview prefetch and media worker on CPU0 at nice `10`.
- catalog worker starts as nice `5` on CPU0, then foreground catalog and walker
  run unpinned at nice `0`.
- preview selected remains unpinned at nice `0`.

Interpretation: first scan is currently the largest production performance
regression. The final 3 second exFAT publish is not the primary problem. The
dominant costs are the full filesystem scan/classification plus RAM catalog
hydration and SQLite/import projection work for a 61k-discovery library.

### Diagnostics Not Run

These were intentionally not run because the deployed production bench-tools
binary did not expose the diagnostics-only commands, and switching binaries
would make the frame/catalog results less comparable:

- `scripts/profile-preview-index-refresh.sh PERF-20260705-IDX`
- `scripts/profile-preview-pack-decode.sh PERF-20260705-PACK --iterations 5 --order random`

Both reported that a diagnostics-capable deployment is required.

### Benchmarks Not Run

I did not run `scripts/device-startup-reveal-acceptance.sh` in this pass. It is
production-relevant, but it performs a cold DB removal/restore sequence plus
return-from-game flow. After the first-scan failure already rebuilt the catalog
and produced the key cold-start regression, running another destructive reboot
acceptance would add risk without changing the main optimization direction.

## Initial Optimization Backlog

Measured top priorities:

1. Fix first-scan scale regression for the current 61k-discovery library.
   Target scan/classify duplication, archive/container classification cost, RAM
   catalog hydration, and projection/import work before focusing on the final
   3-second SQLite publish.
2. Defer or budget warm navigation/full-catalog hydration after reveal. Warm
   first frame is good, but the 1.3-2.4s navigation tail can disrupt early
   interactions and short benchmarks.
3. Add per-frame preview result budgeting/coalescing. The official preview gate
   passes, but turbo preview scroll still attributes real work-budget misses to
   preview scheduling/apply/blit bursts.
4. Repair cold-preview/first-preview benchmark determinism. The helper should
   start on a known preview-bearing system/row and the Neo Geo cold turbo gate
   should not fail because the selected window has no previewable rows.
5. Avoid tiny AmigaVision descriptor rewrites on launch prep. Most structured
   launch refs are microseconds, while descriptor writes cost milliseconds and
   4 KiB exFAT writes for tiny payloads.

Strong code candidates:

1. Overlay-only launcher fast path.
2. Warm-validation snapshot to collapse repeated filesystem work.
3. UI/vsync thread policy plus default thread-sample gate.
4. Launch handoff probe throttling or Main-status replacement.
5. First-scan benchmark cleanup for nav projection.

Worth exploring:

1. Byte/area-aware dirty rect promotion.
2. Deferred full-pack warm while selected preview uses sidecar pread.
3. Batched summary/nav projection publish.
4. Bounded per-frame worker result processing.
5. Direct RGB565 row/filter rendering and selected-strip cache.

Speculative until measured:

1. Dynamic launcher frame order switching.
2. `mmap` for preview packs.
3. Worker pool for cold classification.

Measurement gaps:

1. Diagnostics build pass for preview-index refresh and preview-pack decode.
2. First-scan benchmark cleanup should also remove `library.nav.lz4b` and
   report exactly which artifacts were removed.
3. Launch-handoff script should explain why `--iterations 5` emitted only two
   samples.
4. Add standard columns for present bytes, wasted present bytes, core residency,
   and per-thread CPU deltas to frame summaries.
