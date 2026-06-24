# Production Performance Review - 2026-06-24 - commit 9c9ee441

Scope: production MiSTer MagiK code only. Experimental effects were excluded.
This review has two phases:

- Phase one: static review of current code, plus three parallel read-only
  sub-agent audits for rendering/UI, catalog/media I/O, and benchmark/device
  orchestration.
- Phase two: real hardware benchmarks and profiles on the configured MiSTer at
  `192.168.1.117`.

Commit under test: `9c9ee441`.

Production binary restored after profiling/scene benchmarks:
`mister-magik-fb` launcher-only `release-device`, 6,110,964 bytes, deploy
checksum `e937e28ad010d5a3`.

Final device sanity:

- `MiSTer_MagiK` running.
- `mister-magik-fb` running.
- Framebuffer: RGB565, 960x540, stride 1920.
- Device catalog acceptance: ok.

## Executive Summary

The current production architecture is in much better shape than the earlier
performance notes from the same date. Several large risks have already been
addressed in code:

- Warm startup now uses `library.summary.json` and reaches the first frame in
  about 49 ms.
- Full catalog hydration happens after the first frame and completes around
  0.5 s on the tested catalog.
- Preview loading now has selected and prefetch lanes sharing an archive-backed
  decoded cache.
- Launch handoff is workerized; the UI continues rendering during a simulated
  750 ms slow/failing Main handoff.
- SQLite build/publish uses tmpfs build plus large sequential copy to
  `/media/fat`, which is the right strategy for the SD card.

The biggest remaining optimization opportunities are:

1. Fix the `held-scroll` benchmark harness. It currently produces invalid
   non-moving traces under summary-first startup.
2. Optimize preview fade/blit, especially the generic RGB565 fade path. The CPU
   profile attributes 25% of all samples to
   `blit_transition_565_fade_generic` and 15% to scalar `blend_565`.
3. Reduce benchmark/instrumentation overhead. Per-frame preview trace formatting
   accounts for about 6% of CPU-profile samples.
4. Attack cold catalog scan/classification. First scan is dominated by SD-card
   directory/metadata walking, not by SQLite publish.
5. Investigate media download save path. Isolated Neo Geo screenshot save is
   about 2.0 s, but the full download path reports 8.3 s spent in save.
6. Preserve and strengthen launch-cache warming. Cold generated `.mgl` launch
   materialization is about 266 ms per virtual Neo Geo launch ref; warm cached
   refs are sub-millisecond.

The dual-core Cortex-A9 guidance is stable: keep framebuffer writes and final UI
composition ordered on the UI thread, and spend core two on bounded
preparation: preview decode, fade precompute, row-cache warming, catalog
hydration, launch prep, and low-priority media work.

## Phase One Static Findings

### Rendering Model

The framebuffer ownership model is sound:

- Slint renders into cached RAM.
- Production UI is RGB565.
- Dirty rectangles are copied from cached RAM to `/dev/fb0`.
- Arcade overlays are composed carefully with cached/direct present accounting.
- Vsync waiting has a dedicated worker thread.

The current model should not be replaced with direct Slint rendering into live
framebuffer memory. Multi-threaded `/dev/fb0` writes are also not the next
optimization target; the moving traces show custom draw and fade work dominate
the frame budget before raw present bandwidth does.

### Preview Fade Is The Primary UI CPU Target

Static code review showed two fade paths:

- Same-geometry RGB565 fade uses table-assisted row blending.
- Generic fade falls back to per-pixel scalar `blend_565`.

The hardware CPU profile confirmed this directly:

- `blit_raw_preview_if_needed`: 30.01% of samples.
- `blit_transition_565_fade`: 29.90%.
- `blit_transition_565_fade_generic`: 25.24%.
- `blend_565`: 14.95%.
- `blit_transition_565_fade_same_geometry`: 3.82%.

The current optimization should be to make the production preview path hit the
same-geometry/table path more often, or bring table/row blending to the generic
path.

### Per-Frame CPU/Allocation Work Still Exists

Static review found several second-order hot-path costs:

- Preview prefetch scheduling can run every Arcade frame, including same
  selected-game paths.
- Arcade list rendering hashes visible row strings every frame before deciding
  whether cached surfaces can be reused.
- Input event state formats strings on raw-event changes even when production UI
  does not need them.
- Runtime/status collection copies Slint strings in the frame loop.
- Preview trace logging formats and writes every frame during benchmarks.

The CPU profile makes the last item concrete:

- `LauncherFrameAccounting::finish_frame`: 6.26%.
- `write_preview_trace`: 5.83%.
- `std::io::Write::write_fmt`: 5.83%.

This overhead should be removed from benchmark traces before using profiles for
fine-grained optimization decisions.

### Catalog And SD-Card Work

Current good decisions:

- Scanner avoids screenshot/cache/media trees.
- SQLite build happens in tmpfs for `/media/fat` outputs.
- Final publish is a large sequential copy.
- Screenshot/media packs are pack archives, not many small hot-path files.
- Full ROM hashing is off by default.

Remaining static risks:

- First-scan persistence writes summary immediately after SQLite publish by
  reopening/reloading the just-published DB, then the catalog worker loads the
  DB again before `Ready`.
- ZIP central-directory scanning still does many tiny reads/seeks.
- Saturn/CHD metadata paths can double-open small headers.
- `.mgl` metadata reads use unbounded `read_to_string`, unlike capped MRA reads.
- Media publish/state sync can fall back to global `sync`.
- Stale virtual-launch cache refresh reads existing generated `.mgl` files
  one by one.

### Benchmark Harness Risk

The user spotted this during phase two: `held-scroll` did not scroll.

Root cause from code and trace:

- Summary-first startup marks `catalog_ready=true` with systems but no full game
  rows yet.
- `HeldScroll` starts stepping while `active_game_count` is zero, so it returns
  without initiating a held press.
- The benchmark step counter still advances.
- After full catalog hydration, `HeldScroll` passes `previous_dir=1` because
  step is no longer zero, but the nav state has no active held direction, so no
  press is begun and the list remains at index 0.

Evidence:

- `PERFREVIEW-20260624-PREVIEW-HELD`: 3,595 frames, `moving=0`,
  `fractional_visual_index_frames=0`, max selected 0, max visual index 0.
- `PERFREVIEW-20260624-PREVIEW-TURBO`: 3,597 frames, `moving=3577`, max visual
  index 891.

Do not use held-scroll results from this run as scroll performance evidence.
Do not use `gate-preview-60fps.sh` or `profile-arcade-scroll.sh` as final
evidence until this is fixed, because their held-scroll leg is affected.

## Phase Two Hardware Results

### Preview Scroll And Fade

Artifacts:

- `build/preview-scroll-profiles/PERFREVIEW-20260624-PREVIEW-TURBO-arcade.tsv`
- `build/preview-scroll-profiles/PERFREVIEW-20260624-PREVIEW-STEP-arcade.tsv`
- `build/preview-scroll-profiles/PERFREVIEW-20260624-PREVIEW-CPU-arcade.tsv`
- `build/preview-scroll-profiles/PERFREVIEW-20260624-PREVIEW-CPU-arcade-cpu.svg`
- `build/preview-scroll-profiles/PERFREVIEW-20260624-PREVIEW-HELD-arcade.tsv`
  is invalid as motion evidence.

Valid moving stress run: `turbo-hold`, 60 s.

- Frames: 3,597.
- Moving frames: 3,577.
- p95 work: 4,778 us.
- p99 work: 5,140 us.
- Work frames over 16.667 ms: 1.
- Wall frames over 16.667 ms: 75.
- Vsync fallback/timeout/error: 0.
- Max vsync miss streak: 0.
- Preview loads: archive-backed, no unexpected file reads.
- Cache state frames: exact 3,119, stale 13, placeholder 92.

Turbo custom-draw split:

- `custom_draw_us`: avg 2,306, p95 3,476, p99 3,796.
- `preview_blit_us`: avg 2,026, p95 3,365, p99 3,689.
- `arcade_list_update_us`: avg 270, p95 819, p99 889.
- `cached_present_us`: avg 374, p95 467, p99 482.
- `overlay_present_us`: avg 513, p95 555, p99 584.

Valid lower-velocity preview-change run: `preview-step-hold`, 60 s.

- Frames: 3,596.
- Moving frames: 88.
- p95 work: 335 us.
- p99 work: 5,035 us.
- Work frames over 16.667 ms: 4.
- Wall frames over 16.667 ms: 19.
- Longest observed work spike in script output: about 80 ms.

Interpretation:

- Steady-state moving preview is comfortably under budget.
- The average path is good.
- Rare spikes still exist and need attribution.
- The production CPU hot path during movement is preview fade/blit, not Slint
  renderer or framebuffer present.

### CPU Profile

Artifact:

- `build/preview-scroll-profiles/PERFREVIEW-20260624-PREVIEW-CPU-arcade-cpu.svg`

Run:

- `profile-preview-scroll.sh 30 turbo-hold ... --cpu-profile`.
- Profiling binary was 66.49 MiB.
- Agent deploy hit broken pipe and fell back to device deploy transaction.

Top sampled functions:

- `run_launcher_loop`: 47.72%.
- `blit_raw_preview_if_needed`: 30.01%.
- `blit_transition_565_fade`: 29.90%.
- `blit_transition_565_fade_generic`: 25.24%.
- `blend_565`: 14.95%.
- Preview prefetch thread: 7.64%.
- Preview trace formatting/writing: 5.83%.
- `ArcadeListRenderer::draw`: 4.77%.
- LZ4 decode path: 3.92%.
- Same-geometry table fade: 3.82%.
- Slint software render stack: about 2.97% in this run.

Interpretation:

The optimization order is clear:

1. Reduce generic fade/blend cost.
2. Reduce benchmark trace overhead.
3. Then tune arcade list row/cache work.

### Warm Startup

Artifact:

- `history/toolchain-bench/results-warm-catalog.tsv`

Run:

- `scripts/profile-warm-catalog-start.sh PERFREVIEW-20260624-WARM --replace-label --iterations 5`

Results:

- First frame: 48-49 ms, mean 48.6 ms.
- Summary load: 252-262 us, mean 256.6 us.
- Full catalog ready: 482-498 ms, mean 485.2 ms.
- Full catalog load: 460-477 ms.

Interpretation:

The summary-first path is a major win. It avoids the older synchronous
full-SQLite load before first UI. Keep this design.

### Cold First Scan

Artifact:

- `history/toolchain-bench/results-first-scan.tsv`

Run:

- `scripts/profile-first-scan.sh PERFREVIEW-20260624-FIRST --replace-label --timeout 240`

Results:

- First frame: 95 ms, `catalog_ready=false`.
- First discovery: 2.443 s.
- Walk: 36.121 s.
- File discovery: 34.001 s.
- Classify total: 37.951 s.
- Library scan complete: 39.093 s.
- Metadata load: 2.876 s.
- Insert games total: 4.606 s.
- Import total: 10.053 s.
- SQLite publish: 18,026,496 bytes, 1.369 s total, 1.354 s copy.
- DB saved: 51.743 s.
- Library ready: 52.093 s.
- Final rows: 9,229 games in DB, 7,256 UI games.

Interpretation:

Cold scan is SD-card metadata traversal dominated. SQLite publish is not the
bottleneck. Import is meaningful but second to discovery/classification.

### Hot Library Refresh And Save

Artifacts:

- `history/toolchain-bench/results-library-io.tsv`
- `history/toolchain-bench/results-library-save.tsv`

Hot `library-refresh` profile:

- Total wrapper elapsed: 12 s.
- Scan/classify: 2.734 s.
- Import: 7.177 s.
- Publish: 1.623 s.

Five direct SQLite publish samples for 18,026,496 bytes:

- Total ms: 1,455, 1,461, 1,462, 1,396, 1,519.
- Mean: 1.459 s.
- Copy ms: 1,384-1,507.
- Progress events: 70.

Interpretation:

Hot-cache rebuilds are CPU/import heavy. Cold rebuilds are directory/metadata
walk heavy. The progress-capable sequential publish path is healthy and far
better than older non-progress rows in the same TSV, which were around
5.8-6.3 s.

### Screenshot Media Save And Download

Artifacts:

- `history/toolchain-bench/results-screenshot-save.tsv`
- `history/toolchain-bench/results-screenshot-download.tsv`

Neo Geo direct save benchmark:

- Bytes: 24,283,092.
- Total ms: 1,918, 1,916, 2,490, 1,890, 2,027.
- Mean total: 2.048 s.
- Parent sync: stable at 7 ms.

Neo Geo full download benchmark:

- Cloudflare cache: HIT.
- Encoded/decoded bytes: 24,283,092.
- Download: 6.066 s.
- Save: 8.314 s.
- Verify: 1.323 s.
- Total: 15.704 s.
- Wire throughput: 32.03 Mbps.

Interpretation:

The isolated save path says the card can publish this pack in about 2 s. The
full download path reports 8.3 s in save, so there is a path-specific cost to
investigate: temp location, chunking, flush/sync behavior, interaction with
download stream, verification ordering, or write amplification.

### Launch Preparation And Handoff

Artifacts:

- `history/toolchain-bench/results-launch-prep.tsv`
- `history/toolchain-bench/results-launch-handoff.tsv`
- `build/launch-handoff/PERFREVIEW-20260624-HANDOFF.tsv`

Warm launch prep:

- Samples: 120.
- Errors: 0.
- p50: 92 us.
- p95: 2.898 ms.
- Warm virtual Neo Geo refs generally sub-0.2 ms.
- AmigaVision samples are a few ms because they write `_Computer/Amiga.mgl`.

Cold launch prep:

- Samples: 120.
- Errors: 0.
- p50: 265.990 ms.
- p95: 271.440 ms.
- Cold generated virtual Neo Geo `.mgl` materialization is about 266 ms per
  launch ref.

Simulated slow-fail handoff, 750 ms delay, 5 iterations:

- Loading frame appears in 34.8-48.4 ms.
- Max frame gap: 17.5-29.7 ms.
- Loading frames before result: 47-48.
- Failure recovery: 1.49-1.52 ms.
- UI remains alive during the 750 ms wait.

Interpretation:

The workerized handoff design is working. Cold virtual launch materialization is
still too expensive for an interactive path, so cache preservation and proactive
materialization matter.

### Scene/Toolchain Sanity Bench

Artifact:

- `history/toolchain-bench/results.tsv`

Run:

- `scripts/bench-toolchain.sh PERFREVIEW-20260624-SCENE --replace-label --device --scene-secs 15`
- This builds and deploys the all-scenes binary, not the launcher-only
  production binary.

Results:

- Build: 79.66 s.
- All-scenes binary: 7,494,452 bytes.
- `demo`: pass, 60 fps, render 867 us, copy 426 us, CPU mean 8%.
- `full_motion`: failed scene gate, 60 fps average but `vsync_fallback=14`,
  `visual_ok=no`, `timing_ok=no`.
- `static_ui`: pass, 61 fps, render 1 us, copy 0 us.
- `local_motion`: pass, 61 fps, render 140 us, copy 12 us.

Interpretation:

The generic scene benchmark is not the production launcher acceptance gate, but
`full_motion` should be investigated because vsync fallback should not happen in
the simple all-scenes run.

### Device Acceptance

Run:

- `scripts/device-catalog-acceptance.sh`

Result:

- `device catalog acceptance: ok`.
- Launcher process count: 1.
- No active `library-refresh`.
- `library.sqlite3` present and non-empty.
- `launcher_catalog` table present.
- Preview counts: arcade 892, neogeo 179, saturn 115.
- Runtime-only screenshot asset table count: 0.
- Size-qualified screenshot pack installed.
- Media state checks ok.

## Prioritized Optimization Plan

### P0 - Fix Benchmark Truth

1. Fix `HeldScroll`.

   Options:

   - Do not increment `launcher_bench_step_idx` when `launcher_bench_step`
     returns false because the full game list is not available.
   - Or derive `previous_dir` from `nav.arcade` held state rather than from the
     global step counter.
   - Or make Arcade benchmarks wait for full catalog hydration, not summary-only
     readiness.

2. Make `motion_check` a hard gate for scroll benchmarks.

   A trace with `moving_frames=0` must fail before timing summaries are treated
   as scroll results.

3. Update `gate-preview-60fps.sh` and `profile-arcade-scroll.sh` after the fix.

   Until then, use `turbo-hold` direct runs for moving stress evidence and treat
   `held-scroll` rows as invalid.

### P1 - Optimize Preview Fade/Blit

Best next experiments:

- Normalize preview display geometry so more transitions use
  `blit_transition_565_fade_same_geometry`.
- Extend table-assisted row blending to the generic fade path.
- For clipped/offset preview segments, split into row slices and use
  `blend_565_row` instead of per-pixel `blend_565`.
- Consider off-thread precomputation of short fade frames for selected previews,
  bounded by memory and generation.

Success criteria:

- `turbo-hold` p99 `preview_blit_us` below 1.5 ms.
- `blit_transition_565_fade_generic` no longer dominates the CPU SVG.
- No visual regression in preview centering/cropping.

### P1 - Clean Up Measurement Overhead

Preview trace logging currently costs enough CPU to pollute profiles.

Actions:

- Buffer preview trace rows in memory and flush at end or in coarse chunks.
- Avoid `write_fmt` per frame.
- Record actual copied bytes per rect using framebuffer bytes-per-pixel, not
  `pixels * 4`.
- Add separate cached/direct overlay byte counters.

Success criteria:

- `write_preview_trace` disappears from top CPU profile nodes.
- RGB565 bandwidth reporting matches actual copied bytes.

### P1 - Investigate Rare Frame Spikes

The valid moving runs are comfortably under budget in steady state, but there
are isolated 28-80 ms work spikes.

Actions:

- Add trace fields for spike reason: selected preview apply, LZ4 decode result,
  catalog worker message, media gate event, status write, OS scheduling, and
  malloc/realloc if feasible.
- Add a "log slow frame details only" mode that does not trace every frame.
- Re-run `turbo-hold` after removing trace overhead.

### P1 - Media Download Save Path

Direct save: about 2.0 s for 24.3 MB.

Full download save: 8.3 s for the same size.

Actions:

- Break `media-bench-download` save timing into temp write, final publish,
  fsync, parent sync, state write, and verify reread.
- Compare temp directory and final filesystem.
- Confirm whether the download path uses the same chunk size and publish helper
  as `media-bench-save`.
- Test save-after-download with verification disabled and with delayed verify.

### P1 - Cold Catalog Scan

Cold first scan is dominated by SD-card metadata traversal.

Actions:

- Read ZIP central directories into bounded memory buffers instead of many tiny
  seeks/reads.
- Prefer filename/folder region inference before header reads for extensions
  where offset 0 cannot contain the target boot header.
- Bound `.mgl` reads or parse only the early XML needed for metadata.
- Keep scanner ignore boundaries strict; do not regress into media/cache walks.
- Consider persisting more directory/file signatures so unchanged cold scans can
  skip expensive metadata work after only root stamp validation.

### P2 - SQLite Import And Summary

Import is second-order on cold scan and primary on hot refresh.

Actions:

- Build `library.summary.json` from the already-built in-memory projection or
  from the loaded catalog worker result instead of reopening SQLite immediately.
- Profile metadata load and normalize/title lookup allocations.
- Test secondary indexes only with measured launch/query benefits; do not add
  indexes blindly if import cost grows more than launch prep shrinks.

### P2 - Launch Cache

Cold virtual generated launches are expensive because they create small files on
`/media/fat`.

Actions:

- Treat virtual launch cache as a production artifact to preserve aggressively.
- Add manifest/content hashes so stale refresh can avoid reading every existing
  `.mgl`.
- Materialize popular/visible virtual refs after catalog ready on a low-priority
  worker.
- Do not materialize in the hot launch path when the cache stamp is valid.

### P2 - Use The Second Core For Preparation

Good uses:

- Selected preview decode.
- Prefetch decode.
- Preview fade precompute.
- Arcade row pre-render/cache warming.
- Catalog hydration/validation.
- Launch prep/handoff.
- Media download/checksum/publish when interaction gate permits.

Avoid:

- Concurrent unordered writes to `/dev/fb0`.
- Unbounded decode/import workers.
- Background SD-card work during active scroll or launch.

## Commands Run

Primary production setup:

```bash
scripts/mister status
scripts/deploy-rust.sh --device --ui-scope launcher
```

Preview and CPU:

```bash
scripts/profile-preview-scroll.sh 60 held-scroll PERFREVIEW-20260624-PREVIEW-HELD --skip-build --visual-captures 0
scripts/profile-preview-scroll.sh 60 turbo-hold PERFREVIEW-20260624-PREVIEW-TURBO --skip-build --visual-captures 0
scripts/profile-preview-scroll.sh 60 preview-step-hold PERFREVIEW-20260624-PREVIEW-STEP --skip-build --visual-captures 0
scripts/profile-preview-scroll.sh 30 turbo-hold PERFREVIEW-20260624-PREVIEW-CPU --cpu-profile --visual-captures 0
```

Startup and catalog:

```bash
scripts/profile-warm-catalog-start.sh PERFREVIEW-20260624-WARM --replace-label --iterations 5
scripts/profile-first-scan.sh PERFREVIEW-20260624-FIRST --replace-label --timeout 240
scripts/profile-library-io.sh PERFREVIEW-20260624-LIBIO --replace-label --sample-limit 120
scripts/profile-library-save.sh PERFREVIEW-20260624-LIBSAVE --iterations 5 --replace-label
```

Media:

```bash
scripts/profile-screenshot-save.sh PERFREVIEW-20260624-SHOT-SAVE --system neogeo --iterations 5 --replace-label
scripts/profile-screenshot-download.sh PERFREVIEW-20260624-SHOT-DL --system neogeo --iterations 1 --replace-label
```

Launch:

```bash
scripts/profile-launch-prep.sh PERFREVIEW-20260624-LAUNCH-WARM --replace-label --scenario warm --iterations 10
scripts/profile-launch-prep.sh PERFREVIEW-20260624-LAUNCH-COLD --replace-label --scenario cold --iterations 10
scripts/profile-launch-handoff.sh PERFREVIEW-20260624-HANDOFF --replace-label --iterations 5 --delay-ms 750
```

Supporting scene/device checks:

```bash
scripts/bench-toolchain.sh PERFREVIEW-20260624-SCENE --replace-label --device --scene-secs 15
scripts/deploy-rust.sh --device --ui-scope launcher
scripts/mister run "if [ -p /dev/MiSTer_cmd ]; then printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd; else echo missing_fifo; exit 12; fi"
scripts/mister status
scripts/device-catalog-acceptance.sh
```

## Caveats

- `held-scroll` was invalid and must not be used as scroll evidence.
- `gate-preview-60fps.sh` was not used as final evidence because of the
  held-scroll bug.
- CPU-profile timings are from a profiling binary and should be used for
  attribution, not final product frame-budget numbers.
- Scene/toolchain bench deploys the all-scenes binary and is supporting
  evidence only.
- No experimental effects were reviewed or benchmarked.
