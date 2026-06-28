# Production Performance Review - 2026-06-28

Scope: production MiSTer MagiK code only. Experimental effects were excluded.
The review used four read-only sub-agents for phase one, then real MiSTer
hardware at `192.168.1.117` for phase two.

Revision note: the phase-one and initial phase-two sections below describe the
pre-implementation review baseline. The later implementation pass produced the
commits listed in "Implementation Summary"; their committed evidence supersedes
the baseline rows for the paths they changed.

## Executive Summary

The production rendering architecture is strong. The official 60fps preview
gate passed for both held-scroll and turbo-hold. Slint render, custom preview
fade/blit, and framebuffer present are all comfortably below frame budget in
steady state:

- Held-scroll gate: p99 work `3154us`, all frames paced by vsync, no fallback,
  no timeout, max miss streak `0`.
- Turbo gate: p99 work `3182us`, all frames paced by vsync, no fallback, no
  timeout, max miss streak `0`.
- Scroll present still writes about `704` logical rows per scrolling frame, but
  p99 framebuffer present remains around `1.07-1.19ms`.
- The production preview hot path is correctly archive-backed or indexed:
  first selected preview used `index_pread`, with selected read time `328us`.

The biggest optimization target is no longer raw rendering. It is background
catalog work crossing back into the UI loop as large prepare spikes. Every
Arcade/preview run showed the same shape: steady frames are excellent, then a
catalog-worker event produces one or more half-second UI-thread stalls:

- `PERF20260628-PROD-HELD`: one `catalog_worker_us=661652us` frame.
- `PERF20260628-PROD-TURBO`: one `catalog_worker_us=572499us` frame.
- `PERF20260628-FIRST-PREVIEW`: one `catalog_worker_us=706660us` frame.
- `PERF20260628-ARCADE`: one `prepare_us=553587us` frame, followed by a
  `110367us` prepare frame.
- `PERF20260628-GATE-FADE-TURBO`: two worker-attributed slow frames,
  `494989us` and `61503us`, plus one `16027us` prepare frame.

The second big target is cold and warm catalog hydration:

- Warm restart shows first frame in `22-24ms`, which is excellent.
- Full warm catalog readiness is around `3.55s`.
- Full warm catalog load is around `1.49s`, with catalog construction around
  `1.32s`.
- Cold first scan missed the current RAM-catalog gate:
  `library_ready=43252ms` vs the `41000ms` gate.
- Cold first scan saved the DB at `58097ms`; scan/walk was `35.6s`, import was
  `12.3s`, exFAT publish copy was only `1.6s`.

The exFAT strategy is broadly right: large sequential writes are acceptable, and
small hot-path writes should be avoided. The evidence points to catalog
hydration/materialization and UI-thread delivery as the next high-leverage work.

## Implementation Summary

The production performance plan was implemented as eight logical commits:

- `0484c9e4 Tune production benchmark harness`
  - Kept experimental effect scenes out of default production runs.
  - Standardized short 30s `turbo-hold` scroll options and clearer diagnostic
    failures.
- `145dbcbb Trust generated preview sidecar indexes`
  - Treated generated `.idx` files as trusted after cheap corruption/fingerprint
    checks.
  - Deferred full-pack warm pressure away from first selected preview.
  - Evidence labels used during the device pass: `IDXTRUST-*` first-preview and
    30s turbo preview-scroll runs in `build/first-preview-profiles/` and
    `build/preview-scroll-profiles/`.
- `95d4773e Budget catalog ready swaps during scroll`
  - Deferred full catalog swaps under active Arcade scroll and added deferred
    catalog trace fields.
  - Evidence: `CATBUDGET-BEFORE-20260628` through
    `CATBUDGET5-AFTER-20260628` in
    `history/toolchain-bench/results-warm-catalog.tsv`, plus matching 30s
    turbo preview-scroll profile artifacts.
- `e4d4e2d5 Fix supervised arcade scroll benchmark restart`
  - Fixed the supervised benchmark restart path so later Arcade scroll evidence
    measures the intended launcher process.
- `4fbf6346 Defer navigation text indexes`
  - Deferred full navigation text index construction until needed while keeping
    summary startup usable.
  - Evidence: `CATHYDRATE-BEFORE-20260628` through
    `CATHYDRATE3-AFTER-20260628` in
    `history/toolchain-bench/results-warm-catalog.tsv`; full warm catalog
    readiness dropped from about `3.56s` to about `2.37s`, and construction
    time dropped from about `1.32s` to about `0.14-0.15s`.
- `fa792f1e Accelerate cold catalog publish`
  - Reduced duplicate catalog construction during scan/import and precomputed
    summary/projection payloads before DB publish.
  - Evidence: `COLDSCAN-BEFORE-20260628` /
    `COLDSCAN4-AFTER-20260628` in
    `history/toolchain-bench/results-first-scan.tsv` and
    `history/toolchain-bench/results-library-io.tsv`; `library_ready` moved back
    under the `41000ms` gate, with the final accepted run at about `40.25s`.
- `01c56b00 Trim arcade row redraw churn`
  - Reduced per-redraw row allocation/projection churn in the Arcade renderer.
  - Evidence: `ARCDRAW-BEFORE-20260628` / `ARCDRAW-AFTER-20260628`,
    documented in `history/2026-06-28-arcade-redraw-trim.md`.
- `980e1141 Harden MagiK launch acknowledgements`
  - Required a fresh Main `main-status.json` timestamped handoff acknowledgement
    after MagiK FIFO launch writes.
  - Fixed the auto-launch smoke hook so it waits for hydrated Arcade rows before
    consuming its one-shot request.
  - Evidence: `LAUNCHACK-BEFORE-20260628` /
    `LAUNCHACK-AFTER-20260628` and cold variants, documented in
    `history/2026-06-28-launch-ack-hardening.md`.

Final policy update: short, targeted before/after benchmarks are now documented
in `docs/benchmarking.md`. Scroll changes use a single 30s `turbo-hold` run by
default; non-scroll changes use the smallest benchmark that owns the changed
path.

## Phase One - Static Review

### Rendering And Arcade UI

The current framebuffer model is the right shape:

- Slint renders into a cached RGB565 target.
- The launcher compositor excludes direct overlays from cached copies.
- The arcade list is a custom RGB565 overlay.
- Preview fade/blit has a direct RGB565 row path for native-sized preview
  assets.

Findings:

1. Preview scheduling can do avoidable per-redraw work.
   `magik-gui/src/ui_runner/launcher_loop.rs` calls
   `schedule_arcade_preview_window` while Arcade is dirty, and
   `magik-gui/src/preview_state.rs` rebuilds prefetch index vectors and preview
   key strings. This is not every quiet vsync frame, because it is gated by
   `dirty_opt`, but redraw-driven idle or transition periods can still pay the
   same selection/window checks.

   Optimization: cache the selected index plus catalog/list generation and skip
   prefetch coverage checks until selection, direction, catalog generation, or
   pending-result state changes. Prefer borrowed key comparisons before building
   `String`s.

2. Arcade renderer visible-window hashing is still string-heavy.
   `magik-gui/src/arcade_list_renderer.rs` computes a visible-window hash for
   unchanged positions so it can detect content mutation. That hashes visible
   row title/path/archive fields even when no catalog mutation is possible.

   Optimization: pass a catalog/list generation into the renderer and only
   recompute visible content hashes after generation changes.

3. Row-cache misses allocate two row-sized buffers.
   Uncached rows render into a `Vec<Pixel>`, clip into a new `String`, then
   convert to a second `Vec<Rgb565Pixel>`. This is fine once warm, but first
   long scrolls can allocate and copy in bursts.

   Optimization: render directly into reusable RGB565 row buffers, avoid
   `clipped_title` allocation, and optionally pre-render ahead of the scroll
   direction on the second Cortex-A9 core.

4. Scroll present still rewrites most of the list overlay.
   The live framebuffer is not read or scrolled, which is good, but the list
   overlay still presents nearly the full `464x384` area each scroll frame.

   Optimization: keep the current default, but test two opt-in variants:
   larger contiguous writes for wrapped surface chunks, and a dual-worker
   RGB565 band copy. The dual-core version must stay behind an env flag until
   measured, because write-combined mmap contention may lose.

5. Scaled preview blit divides per output pixel on non-native preview sizes.
   Production should keep preview packs close to display size. If scaling
   remains needed, precompute source-x/source-y lookup tables per preview
   geometry or add integer-scale row expanders.

### Catalog, Storage, Preview Packs

The scanner already avoids the largest historical traps:

- Scan roots are narrowed to launcher and game directories.
- Screenshot/media/cores paths are ignored.
- SQLite builds under `/tmp` for production `/media/fat` DBs, then publishes a
  large sequential copy to exFAT.
- Runtime preview loading uses raw565 packs and `.mmlz4b.idx` sidecars.

Findings:

1. Sidecar index loading may still validate too much on first use.
   `magik-gui/catalog/src/preview_worker.rs` reads `.idx`, then validates
   against embedded archive entries. If this re-reads every embedded entry, the
   first indexed preview becomes O(pack entries), weakening the `pread` fast
   lane on exFAT.

   Optimization: trust the sidecar when archive size/mtime plus manifest or
   state metadata match. Defer full validation to idle/background verification.

2. First indexed preview starts a full archive warm in the background.
   After indexed `pread` succeeds, the loader starts background full-pack load.
   That is good for steady scroll, but it can compete with first interaction
   and catalog hydration.

   Optimization: idle-gate full-pack warm-up after first selected preview, or
   delay it until the UI has no catalog hydration delivery pending.

3. Catalog progress coalescing is too chatty for known percentages.
   `CatalogProgressCoalescer::should_send` treats known percent progress as a
   phase change, weakening the throttle. During cold scan/import this can
   create avoidable worker messages and UI property churn.

   Optimization: throttle unless title/phase changes or percent crosses a real
   delta, for example 1 percent.

4. Preview-index refresh shape is O(systems x rows).
   The refresh loops installed packs and updates materialized tables per
   system without supporting indexes on `system_id, preview_asset_key`.

   Optimization: refresh all systems in one pass using a temp
   `(system_id, asset_key)` table, or add measured indexes if the extra DB size
   is justified.

### Launch And Runtime

Findings:

1. Launch completion is inferred too narrowly.
   Rust treats launch as complete when Main cmdline contains `.rbf` and not
   `menu.rbf`. Main already exposes richer launcher/handoff states. A valid
   launch not detected by this heuristic can leave Slint alive until timeout,
   then route recovery can steal display back.

   Optimization: use Main `HandoffToGame` / launch events as primary success,
   cmdline as fallback. On timeout, do not reassert the launcher route unless
   Main is still launcher-active.

2. FIFO write is treated as launch acceptance.
   `execute_game_launch_with` returns OK after writing `/dev/MiSTer_cmd`, but
   Main can reject a command when not in `LauncherActive`.

   Optimization: add a command id/ack or poll Main status for accepted,
   handoff, or rejected state after FIFO write.

3. Controller event drain is unbounded per frame.
   Each frame drains pad events until `WouldBlock` across devices. Noisy
   controllers or hotplug bursts can become prepare-phase spikes.

   Optimization: cap events or time per frame, carry remaining events forward,
   and log event count plus `input_poll_us`.

4. Recovery documentation and tool behavior diverge.
   `scripts/mister recover` is documented as a recovery command, but the tool
   currently only supports dry-run behavior.

   Optimization: implement a conservative recovery ladder or update docs to
   state the command is diagnostic-only.

### Build And Benchmark Hygiene

Findings:

1. `scripts/bench-toolchain.sh` defaults to `--all-scenes`, which enables the
   `experiments` feature. That makes it unsuitable as a default production
   benchmark command.

   Optimization: make production `--device` the default and require explicit
   `--all-scenes` for lab/effect runs.

2. `--ui-scope launcher|arcade` is documented as a reducer, but currently
   compiles more than the docs imply. Clarify the flag or make it actually
   filter generated Slint sources.

3. Some benchmark commands are exposed in the production binary, while others
   are diagnostics-only and scripts assume they exist. Current production
   binary exposes `media-bench-download`, `media-bench-save`, `library-sql`,
   and `launch-prep-bench`; it does not expose
   `preview-index-refresh-bench`.

   Optimization: align scripts with features through preflight checks and clear
   build hints. Do not silently benchmark a different binary.

## Phase Two - Hardware Evidence

### Production Build And Deploy

Command:

```bash
scripts/profile-preview-scroll.sh 60 held-scroll PERF20260628-PROD-HELD --deploy-device --visual-captures 0
```

Build/deploy evidence:

- Profile: `release-device`
- Features: `ui`
- UI scope: `launcher`
- Binary size: `6,627,212` bytes
- Deploy path: agent deploy to `/media/fat/mister-magik/mister-magik-fb`

### Preview 60fps Gate

Commands:

```bash
scripts/gate-preview-60fps.sh PERF20260628-GATE --skip-build --visual-captures 0
```

Held-scroll leg:

- Frames after frame 30: `3407`
- p99 work: `3154us`
- `work_gt_16_7ms`: `1`
- Vsync/fallback/timeout/error: `3407/0/0/0`
- Max vsync miss streak: `0`
- p95 custom draw: `1599us`
- p95 cached present: `465us`
- p95 arcade list present: `560us`
- Slow frame: `catalog_worker_us=659042us`

Turbo-hold leg:

- Frames after frame 30: `3413`
- p99 work: `3182us`
- `work_gt_16_7ms`: `3`
- Vsync/fallback/timeout/error: `3413/0/0/0`
- Max vsync miss streak: `0`
- p95 custom draw: `1614us`
- p95 cached present: `466us`
- p95 arcade list present: `564us`
- Slow frames: `catalog_worker_us=494989us` and `61503us`; one additional
  prepare-dominant `16027us` frame.

Result: gate passed. The remaining issue is isolated background delivery
outliers, not steady renderer budget.

### Arcade Scroll Cross-Check

Command:

```bash
scripts/profile-arcade-scroll.sh 30 PERF20260628-ARCADE --skip-build
```

Key scroll-frame p99 values:

- Prepare p99: `599us`, max `553587us`
- Slint render p99: `295us`
- Custom draw p99: `1706us`
- Framebuffer present p99: `1189us`
- Cached present p99: `535us`
- Arcade list present p99: `645us`
- Rows p99: `704`
- Vsync path: pass, no fallback/timeout/error

Worst frames:

- Frame 237: `prepare_us=553587`, wall `555910us`
- Frame 238: `prepare_us=110367`, wall `112204us`

Interpretation: steady scroll is healthy. The large outlier is not the list
renderer or framebuffer copy; it is background catalog delivery into prepare.

### First Preview

Command:

```bash
scripts/profile-first-preview.sh PERF20260628-FIRST-PREVIEW --skip-build
```

Selected preview evidence:

- `decoded_load_source=index_pread`
- `apply_load_source=index_pread`
- `decoded_queue_age_us=4879`
- `apply_age_us=216610`
- `decoded_total_us=19198`
- `decoded_read_us=328`
- `decoded_decode_us=8690`
- `decoded_encoded_bytes=63424`

The first selected preview fast lane works. The later slow frame in this run
was again catalog-worker attributed: `catalog_worker_us=706660us`.

### Warm Catalog Startup

Command:

```bash
scripts/profile-warm-catalog-start.sh PERF20260628-WARM --replace-label --iterations 3
```

Rows:

```text
iteration first_frame_ms full_catalog_ready_ms full_catalog_ready_load_us catalog_us result
1         24             3556                  1491342                    1315474   ok
2         22             3554                  1497045                    1322409   ok
3         24             3556                  1496351                    1320000   ok
```

Interpretation:

- The summary/first-frame path is excellent.
- Full catalog hydration is expensive enough that UI-thread handoff must be
  incremental or sliced.

### First Scan

Command:

```bash
scripts/profile-first-scan.sh PERF20260628-FIRST-SCAN --skip-build --replace-label --timeout 240
```

Result: failed the current RAM catalog gate.

Key rows:

- `library_ready=43252ms` with `games=7259 load_us=6539175`
- Gate: `43252ms > 41000ms`
- `library_db_saved=58097ms`
- `scan_us=35578142`
- `discover_us=34824827`
- `classify_us=35577112`
- `import_us=14848646`
- `library_sqlite_publish`: `bytes=19431424 copy_ms=1600 total_ms=1620`
- `build_saved_catalog=1716268us`

Interpretation:

- Cold scan is dominated by filesystem walk/discovery.
- Import/materialization is the next largest cost.
- exFAT publish is visible but not the main problem.

### Library Save And I/O

Commands:

```bash
scripts/profile-library-save.sh PERF20260628-LIBSAVE --iterations 2 --replace-label
scripts/profile-library-io.sh PERF20260628-LIBIO --replace-label --sample-limit 120
```

Library save publish rows:

```text
iteration bytes    copy_ms total_ms progress_events result
1         19431424 1733    1746     76              bench-ok
2         19431424 1792    1805     76              bench-ok
```

Library I/O warm refresh:

- `scan_us=2698957`
- `discover_us=2056828`
- `classify_us=2576081`
- `import_us=8132281`
- `sqlite_publish copy_ms=1582 total_ms=1595`
- Full run completed in `11s`

Important warm import stages:

- Metadata load: `1168ms`
- Insert games total: `2029ms`
- Materialize arcade UI: `505ms`
- Insert launcher console: `278ms`
- Insert launcher launch plans: `671ms`
- Build saved catalog: `861ms`

Interpretation:

- Warm path is CPU/import/materialization heavy.
- `/media/fat` publish remains a bounded sequential write.
- Further exFAT micro-optimizing is lower value than reducing import and
  catalog construction work.

### Launch Preparation

Commands:

```bash
scripts/profile-launch-prep.sh PERF20260628-LAUNCH-WARM --replace-label --scenario warm --iterations 5
scripts/profile-launch-prep.sh PERF20260628-LAUNCH-COLD --replace-label --scenario cold --iterations 3
```

Summaries:

- Warm: `count=60`, `errors=0`, `p50_us=28`, `p95_us=2207`
- Cold: `count=36`, `errors=0`, `p50_us=27`, `p95_us=2547`

Interpretation:

- Launch preparation itself is not slow.
- The launch/runtime concern is acknowledgement and recovery correctness, not
  prep CPU cost.

### Preview Index Refresh

Command attempted:

```bash
scripts/profile-preview-index-refresh.sh PERF20260628-PREVIEW-INDEX
```

Result: failed because the production binary does not expose the diagnostics
command:

```text
unknown command 'preview-index-refresh-bench'
```

Interpretation: the static finding stands. The benchmark script needs a
diagnostics preflight or the command needs to move into the production benchmark
surface if it is required for release evidence.

## Priority Optimization Backlog

### P0 - Stop Catalog Worker Spikes From Blocking Arcade Frames

Problem: worker results are delivered to the launcher in large chunks. Steady
rendering is fine, but catalog hydration readiness can block prepare for
`0.5-0.7s`.

Likely work:

- Make catalog worker delivery incremental: systems first, then rows in bounded
  chunks.
- Add a per-frame budget for applying catalog worker messages.
- Move expensive `ArcadeCatalog` construction or projection conversion fully
  off the UI thread.
- Keep current catalog visible until the next catalog is fully staged, then
  swap a cheap `Arc` pointer or small handle on the UI thread.
- Add a benchmark gate that fails on any prepare frame above a high threshold,
  not just p99. The current gate passes despite visible half-second outliers.

Validation:

```bash
scripts/profile-preview-scroll.sh 60 held-scroll LABEL --skip-build --visual-captures 0
scripts/profile-preview-scroll.sh 60 turbo-hold LABEL --skip-build --visual-captures 0
scripts/profile-arcade-scroll.sh 30 LABEL --skip-build
```

Success: no `catalog_worker_us` outliers above 10ms, p99 work still below
14.5ms, no vsync fallback/timeout/error.

### P1 - Reduce Full Catalog Hydration And Construction

Problem: warm full catalog readiness is `3.55s`, load is `1.49s`, and catalog
construction is about `1.32s`.

Likely work:

- Treat `library.summary.json` / navigation projection as the primary warm UI
  seed.
- Hydrate per-system games lazily when the user enters a system or Arcade.
- Store compact row projections in a cheaper binary format or mmap-friendly
  format for launcher navigation.
- Avoid constructing all `ArcadeGameEntry` strings when only Home/system counts
  are needed.

Validation:

```bash
scripts/profile-warm-catalog-start.sh LABEL --replace-label --iterations 5
scripts/profile-preview-scroll.sh 60 held-scroll LABEL --skip-build --visual-captures 0
```

Success: first frame stays below 50ms, full catalog delivery does not create
prepare spikes, and Arcade scroll starts with real hydrated rows.

### P1 - Cold Scan Gate Recovery

Problem: first scan missed the RAM-catalog gate at `43.252s`; final DB save was
`58.097s`.

Likely work:

- Separate early usable catalog from full catalog more aggressively.
- Revisit directory walk ordering so high-value launcher roots are discovered
  earlier.
- Cache or precompute expensive metadata inputs where possible.
- Consider parallelizing independent root scans across the two A9 cores, but
  only with bounded queueing because exFAT metadata I/O can serialize.

Validation:

```bash
scripts/profile-first-scan.sh LABEL --skip-build --replace-label --timeout 240
scripts/profile-library-io.sh LABEL --replace-label --sample-limit 120
```

Success: `library_ready < 41000ms`, no UI stalls during delivery, no increase in
exFAT write risk.

### P1 - Preview Pack Index Trust And Warm Scheduling

Problem: first preview uses `index_pread` successfully, but code shape suggests
first-use validation and immediate full-pack warm can still compete with UI
work.

Likely work:

- Trust sidecar indexes when archive fingerprint and media state match.
- Defer embedded validation to idle.
- Delay full archive warm until after catalog hydration has delivered.
- Add explicit trace rows for sidecar validation time and background pack-warm
  start/end.

Validation:

```bash
scripts/profile-first-preview.sh LABEL --skip-build
scripts/profile-preview-scroll.sh 60 held-scroll LABEL --skip-build --visual-captures 0
```

Success: selected preview remains `index_pread`, selected apply under 250ms,
and no immediate post-preview background I/O frame spike.

### P2 - Trim Per-Redraw Arcade Work

Problem: steady frame budget is good, but there is still avoidable small work:
preview scheduling checks, visible-row hashing, and row-cache allocation on
cold scroll.

Likely work:

- Add catalog/list generation to skip unchanged visible hash recomputation.
- Cache prefetch window coverage by selected index and direction.
- Render row cache entries directly to RGB565.
- Pre-render rows ahead on a low-priority worker.

Validation:

```bash
scripts/profile-arcade-scroll.sh 30 LABEL --skip-build
scripts/profile-preview-scroll.sh 60 turbo-hold LABEL --skip-build --visual-captures 0
```

Success: lower `arcade_list_update_us` and prepare p95 without increasing
present rows or present time.

### P2 - Runtime Launch Acknowledgement

Problem: launch prep is fast, but FIFO write and success detection are not
robust enough.

Likely work:

- Add command ids/acks in Main status.
- Poll Main state after FIFO write.
- Use Main handoff state as success.
- Avoid route recovery if Main is no longer launcher-active.

Validation:

```bash
scripts/profile-launch-prep.sh LABEL --replace-label --scenario warm --iterations 5
scripts/device-launch-return-smoke.sh
```

Success: normal launch exits Slint quickly; rejected launch recovers in about
1s; timeout cannot steal display from a running core.

### P2 - Benchmark Tooling Alignment

Problem: some scripts benchmark diagnostics or experiments without clear
feature alignment.

Likely work:

- Make `bench-toolchain.sh` production by default.
- Add preflight to diagnostics-only scripts.
- Add trace-duration gates to preview/arcade scripts so truncated runs fail.
- Keep `preview-index-refresh-bench` either diagnostics-explicit or production
available.

Validation:

```bash
scripts/gate-preview-60fps.sh --self-test
scripts/profile-preview-index-refresh.sh LABEL
```

Success: production evidence cannot accidentally include experiment code, and
missing diagnostics commands fail with a clear message.

## Artifacts

Key artifacts produced by this review:

- `build/preview-scroll-profiles/PERF20260628-PROD-HELD-arcade.tsv`
- `build/preview-scroll-profiles/PERF20260628-PROD-TURBO-arcade.tsv`
- `build/preview-scroll-profiles/PERF20260628-FIRST-PREVIEW-arcade.tsv`
- `build/preview-scroll-profiles/PERF20260628-GATE-FADE-VEL-arcade.tsv`
- `build/preview-scroll-profiles/PERF20260628-GATE-FADE-TURBO-arcade.tsv`
- `build/arcade-scroll-profiles/PERF20260628-ARCADE-arcade-scroll.tsv`
- `build/warm-catalog/PERF20260628-WARM-1.log`
- `build/warm-catalog/PERF20260628-WARM-2.log`
- `build/warm-catalog/PERF20260628-WARM-3.log`
- `history/toolchain-bench/results-warm-catalog.tsv`
- `history/toolchain-bench/results-library-save.tsv`
- `history/toolchain-bench/results-library-io.tsv`
- `history/toolchain-bench/results-launch-prep.tsv`

The initial baseline first-scan failure output is captured in this report. Later
cold-scan implementation runs appended accepted evidence to
`history/toolchain-bench/results-first-scan.tsv`.
