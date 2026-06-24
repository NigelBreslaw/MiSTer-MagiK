# Production Performance Review - 2026-06-24

Scope: production MiSTer MagiK code only. Experimental effect scenes and
preview-transition experiments were excluded. Review covered current code,
current benchmark policy, and real MiSTer hardware runs on the configured device
at `192.168.1.117`.

Commit under test: `86821977`.

## Executive Summary

The production framebuffer/rendering architecture is sound. The standalone
renderer scenes all pass at 60fps, and idle/static UI correctly falls to near
zero render and present work. The real Arcade launcher path also passes the
60fps preview gate, with zero true work misses, zero vsync fallback, zero vsync
timeouts, and zero vsync errors.

The biggest optimization opportunities are not in generic Slint rendering. The
hot areas are:

- Arcade custom draw/composition during scrolling and fade.
- Cold catalog scan/classification and SQLite import.
- Blocking work that still runs on the launcher thread, especially launch
  handoff and synchronous cached catalog hydration.
- Background media work competing with UI and SD-card bandwidth.
- Benchmark/tooling drift around removed production commands.

The dual-core Cortex-A9 should be used conservatively: keep all framebuffer
present and Slint operations on the UI thread, but move blocking launch prep,
selected-preview decode, catalog hydration, and low-priority prefetch/media work
onto bounded worker lanes. The SD-card strategy should stay "large sequential
reads/writes are acceptable, small hot-path writes are suspect."

## Phase One: Static Review

### 1. Arcade Scroll Hot Path Is Custom Draw, Not Slint Render

Evidence:

- `profile-arcade-scroll.sh 60 PERFAUDIT-20260624-ARCADE --skip-build`
- Trace: `build/arcade-scroll-profiles/PERFAUDIT-20260624-ARCADE-arcade-scroll.tsv`

Arcade held-scroll:

- `custom_draw_us`: p50 4307us, p95 12018us, p99 12160us.
- `slint_render_us`: p50 159us, p95 191us, p99 198us.
- `fb_present_us`: p50 934us, p95 1032us, p99 1069us.
- Scroll frames present about 704 logical rows.

Relevant code:

- `magik-gui/src/arcade_list_renderer.rs:92` builds the draw key and decides
  whether the list surface can be reused.
- `magik-gui/src/arcade_list_renderer.rs:129` advances the cached list surface
  and redraws only newly exposed bands.
- `magik-gui/src/arcade_list_renderer.rs:226` blits cached rows into the list
  surface.
- `magik-gui/src/arcade_list_renderer.rs:302` copies the list overlay into the
  target surface.

Assessment:

The cached list surface is the right shape, but the p95 custom-draw budget is
already close to the 16.667ms frame budget once prepare and present are added.
This is the main 60fps headroom target.

Optimization direction:

- Split custom draw timing further into list band update, preview fade/blend,
  row-cache render, and target overlay copy.
- Warm or pre-render row cache further ahead in scroll direction on a
  low-priority worker, but keep final target composition on the UI thread.
- Consider a smaller copied overlay area for steady scroll if visual layout
  allows it; current traces show full 704-row presentation for scroll frames.
- Keep the current cached-RAM present model. Do not parallelize writes to
  `/dev/fb0`; the measured issue is custom draw/composition headroom, not raw
  present time.

### 2. Preview Loader Is Single-Lane

Relevant code:

- `magik-gui/catalog/src/preview_worker.rs:270` creates one `preview-loader`
  thread.
- `magik-gui/catalog/src/preview_worker.rs:364` processes selected and prefetch
  requests in the same loop.
- `magik-gui/catalog/src/preview_worker.rs:447` sorts one queue by priority.
- `magik-gui/catalog/src/preview_worker.rs:678` supports archive warmup.
- `magik-gui/catalog/src/preview_worker.rs:1232` preloads archive bytes into
  memory.
- `magik-gui/catalog/src/preview_worker.rs:1241` decodes LZ4/raw565 entries.

Hardware evidence:

- Held-scroll preview gate decoded 891 rows, average decode/load about 2976us,
  max 31642us, with zero unexpected file reads.
- Turbo gate decoded 1769 rows, average decode/load about 2555us, max 24366us,
  with zero unexpected file reads.

Assessment:

Archive preloading is doing its job: the hot path is RAM-backed, not random SD
card reads. However selected previews and prefetch still share the same worker.
Under high churn, selected preview latency can inherit prefetch work.

Optimization direction:

- Add a dedicated selected-preview lane and a separate low-priority prefetch
  lane.
- Keep prefetch bounded and cancellable by generation.
- Preserve one global archive memory cache so the two lanes do not duplicate the
  30-40MB archive resident set.

This is one of the cleanest dual-core wins.

### 3. Cached Catalog Load Still Blocks Startup Before The Main Loop

Relevant code:

- `magik-gui/src/ui_runner/launcher_loop.rs:285` synchronously loads the SQLite
  catalog before the interactive launcher loop.
- `magik-gui/src/ui_runner/launcher_loop.rs:384` synchronizes bridge systems
  after that load.
- `magik-gui/catalog/src/sqlite_catalog.rs:308` opens the catalog and loads
  materialized rows.
- `magik-gui/catalog/src/sqlite_catalog.rs:320` queries materialized UI or
  launcher catalog rows.
- `magik-gui/catalog/src/sqlite_catalog.rs:331` derives systems from loaded
  games.

Hardware evidence:

- First-scan run reached first visible frame in 126ms, which is excellent.
- Cold library-ready took 56148ms.
- The final SQLite catalog load after build was 382382us.

Assessment:

The splash path is good, but ready-cache startup still loads and constructs the
full catalog synchronously before normal interaction. That is acceptable today,
but it is the next place to make warm boot feel instant as the catalog grows.

Optimization direction:

- Add a compact home/system projection table that can render Home immediately.
- Hydrate full game rows in a background catalog loader.
- Keep the existing delayed stamp validation behavior; do not regress to
  scanning before first UI.

### 4. Launch Handoff Blocks The Launcher Thread

Relevant code:

- `magik-gui/src/ui_runner/launcher_loop.rs:1270` sets loading UI.
- `magik-gui/src/ui_runner/launcher_loop.rs:1285` forces one loading frame.
- `magik-gui/src/ui_runner/launcher_loop.rs:1300` calls
  `launcher::execute_game_launch(&mra)` synchronously.

Assessment:

The code does the right visual thing first: present a loading frame. But after
that, the launcher thread can block on launch preparation, Main/FIFO waits, and
failure recovery. That is not visible in the scroll benchmarks, but it is a core
production use case.

Optimization direction:

- Move launch prep/handoff into a small worker/state machine.
- Keep UI alive with loading progress and failure recovery.
- Only let the worker own blocking Main/FIFO waits; keep framebuffer recovery
  actions coordinated through the launcher loop.

This is another good dual-core use: it improves worst-case launch UX without
touching the framebuffer present path.

### 5. SQLite Publish Is Sequential And Reasonable, But Cold Build Is Dominated
By Scan/Classify And Import

Relevant code:

- `magik-gui/catalog/src/sqlite_catalog.rs:606` starts SQLite save with stamp
  and progress.
- `magik-gui/catalog/src/sqlite_catalog.rs:616` loads discovery history from
  the previous DB.
- `magik-gui/catalog/src/sqlite_catalog.rs:702` publishes the temp DB.
- `magik-gui/catalog/src/sqlite_catalog.rs:719` copies temp DB to final temp
  path.
- `magik-gui/catalog/src/sqlite_catalog.rs:734` renames final temp into place.

Hardware evidence:

First scan:

- First visible frame: 126ms.
- `library_scan_complete`: 42981ms.
- `library_db_saved`: 55765ms.
- `library_ready`: 56148ms.
- DB rows: 9229 games.
- SQLite publish: 18,026,496 bytes, 1442ms copy, 1462ms total.

Repeated library save:

- Iteration 1 publish: 1942ms total.
- Iteration 2 publish: 1370ms total.
- Iteration 3 publish: 1496ms total.
- Full repeated refresh summaries were about 14.0s, with scan/classify around
  3.4-3.6s and import around 6.8-6.9s.

Assessment:

The current tempfs-build plus sequential publish model is appropriate for
FAT/exFAT. The large write itself is not the main problem. The cold first scan
is much slower than repeated refresh because cold SD-card directory traversal
and metadata access dominate.

Optimization direction:

- Preserve tempfs SQLite build and atomic publish.
- Focus cold-scan work on directory traversal, metadata DB loading, and import
  table materialization.
- Avoid adding small writes during scan progress.
- Consider storing a compact source-root directory manifest only if it can be
  updated outside the hot path and does not undermine the current root-stamp
  simplicity.

### 6. Media Worker Can Compete With UI And SD-Card Bandwidth

Relevant code:

- `magik-gui/src/ui_runner/launcher_loop.rs:1683` starts/seeds screenshot media
  work for catalog systems after first render.

Hardware evidence:

Pure NeoGeo screenshot save:

- 24,283,092 bytes.
- Iterations: 2016ms, 1937ms, 2009ms.

End-to-end NeoGeo screenshot download:

- 24,283,092 bytes.
- Cloudflare cache: HIT.
- Download: 6188ms.
- Save: 8302ms.
- Verify: 1289ms.
- Total: 15780ms.

Assessment:

Pure sequential save is around 2s, but the end-to-end download benchmark shows
the media path can occupy disk/network/CPU for much longer. This should remain
idle/background work and should back off while the user is launching or actively
scrolling.

Optimization direction:

- Limit device-side media downloads to one at a time unless hardware evidence
  shows concurrency helps.
- Pause media save/publish while launching or under sustained Arcade scroll.
- Keep media checks after first frame, not before first UI.

### 7. Benchmark Tooling Drift Needs Cleanup

Evidence:

- `scripts/profile-library-io.sh PERFAUDIT-20260624-LIBIO --replace-label`
  produced only an initial sample.
- `scripts/bench-library.sh` confirmed the production binary no longer exposes
  `library-scan-bench`:

  `unknown command 'library-scan-bench' (use: early-black | ui | experiment-capabilities | library-refresh | media-bench-download | media-bench-save)`

Assessment:

The production command surface intentionally removed low-level commands, but
the benchmark scripts still assume one of them exists. This can silently turn
storage/CPU profiles into non-evidence.

Optimization direction:

- Update `profile-library-io.sh` to run `library-refresh` with disposable DB
  paths instead of `library-scan-bench`.
- Update docs that still list `bench-library.sh` as a valid current production
  benchmark, or make `bench-library.sh` use a diagnostic build explicitly.

## Phase Two: Hardware Results

### Renderer/Toolchain Scenes

Command:

```bash
scripts/bench-toolchain.sh PERFAUDIT-20260624 --replace-label --device --scene-secs 15
```

All rows had `visual_ok=yes`, `timing_ok=yes`, and `capture_ok=yes`.

| Scene | FPS | Slint Render | FB Present | Rows Avg | CPU Mean |
| --- | ---: | ---: | ---: | ---: | ---: |
| demo | 60 | 892us | 418us | 310 | 8% |
| full_motion | 60 | 879us | 418us | 310 | 8% |
| static_ui | 60 | 1us | 0us | 0 | 0% |
| local_motion | 61 | 143us | 13us | 48 | 1% |

Conclusion: generic Slint render and framebuffer present are not the main
production bottlenecks.

### Arcade Held Scroll

Command:

```bash
scripts/profile-arcade-scroll.sh 60 PERFAUDIT-20260624-ARCADE --skip-build
```

Key held-scroll metrics:

- Frames: 3597.
- `custom_draw_us`: p50 4307us, p95 12018us, p99 12160us.
- `slint_render_us`: p50 159us, p95 191us, p99 198us.
- `fb_present_us`: p50 934us, p95 1032us, p99 1069us.
- `wall_us`: p50 16357us, p95 16474us, p99 16904us.

Conclusion: Arcade browsing is healthy but has tight headroom in custom draw.

### Preview 60fps Gate

Command:

```bash
scripts/gate-preview-60fps.sh PERFAUDIT-20260624 --skip-build --visual-captures 0
```

Gate result: passed.

Held-scroll:

- Frames after 30: 3569.
- p99 work: 13585us.
- `work_gt_16667`: 0.
- vsync fallback/timeout/error: 0/0/0.
- max miss streak: 0.

Turbo-hold:

- Frames after 30: 3568.
- p99 work: 13686us.
- `work_gt_16667`: 0.
- vsync fallback/timeout/error: 0/0/0.
- max miss streak: 0.

Combined:

- Frames after 30: 7137.
- p95 work: 13405us.
- p99 work: 13656us.
- `work_gt_16667`: 0.

Conclusion: production preview fade currently meets the 60fps gate. The
remaining issue is headroom, not correctness.

### First Scan

Command:

```bash
scripts/profile-first-scan.sh PERFAUDIT-20260624-FIRST --deploy-device --replace-label --timeout 240
```

Key timings:

- First frame: 126ms.
- Bootstrap counter climb: 366ms.
- Sustained bootstrap counter climb: 781ms.
- Full scan counter climb: 11749ms.
- Library scan complete: 42981ms.
- SQLite publish: 1462ms.
- Library DB saved: 55765ms.
- Library ready: 56148ms.
- DB count: 9229 games.

Conclusion: first UI is fast, but first catalog readiness is near the 60s gate.
Cold scan/classify and import dominate.

### Library Save

Command:

```bash
scripts/profile-library-save.sh PERFAUDIT-20260624-LIBSAVE --iterations 3 --replace-label
```

Publish rows:

- 18,026,496 bytes, 1942ms total.
- 18,026,496 bytes, 1370ms total.
- 18,026,496 bytes, 1496ms total.

Conclusion: the final DB publish is acceptable and sequential. Do not optimize
this before scan/import unless future evidence changes.

### Screenshot Save And Download

Commands:

```bash
scripts/profile-screenshot-save.sh PERFAUDIT-20260624-SHOT-SAVE --system neogeo --iterations 3 --replace-label
scripts/profile-screenshot-download.sh PERFAUDIT-20260624-SHOT-DL --system neogeo --iterations 1 --replace-label
```

Save-only:

- 24,283,092 bytes, 2016ms.
- 24,283,092 bytes, 1937ms.
- 24,283,092 bytes, 2009ms.

Download:

- 24,283,092 bytes.
- Cloudflare cache HIT.
- Download 6188ms.
- Save 8302ms.
- Verify 1289ms.
- Total 15780ms.

Conclusion: media update should stay background and throttled. The save-only
path is much cheaper than the end-to-end download path.

### Device Acceptance

Command:

```bash
scripts/device-catalog-acceptance.sh
```

Result: passed.

Notable checks:

- Exactly one launcher process.
- No active `library-refresh`.
- `library.sqlite3` present and non-empty.
- `launcher_catalog` table present.
- Installed preview packs project nonzero `has_preview` counts.
- Runtime screenshot packs are not indexed as asset tables.

## Prioritized Optimization Backlog

### P0 - Fix Benchmark Tooling Drift

Update `profile-library-io.sh` and `bench-library.sh` so they either use the
current production command surface or explicitly build/use a diagnostic binary.
Today they can produce misleading non-evidence.

Acceptance:

- `profile-library-io.sh LABEL --replace-label` records samples plus a done row.
- It runs against `library-refresh` and a disposable SQLite output path.
- Docs no longer advertise unsupported production commands.

### P1 - Instrument Arcade Custom Draw

Split `custom_draw_us` into sub-phases:

- list band update,
- row render/cache miss,
- list overlay copy,
- preview fade/blend,
- selection frame/direct overlay,
- any bridge/model work counted nearby.

Acceptance:

- The preview gate report can show p95/p99 for each custom-draw sub-phase.
- No extra per-frame heap allocation in normal profiling-off builds.

### P1 - Add Selected Preview Lane

Create separate selected and prefetch preview workers sharing the same archive
cache.

Acceptance:

- Selected preview requests cannot wait behind lower-priority prefetch decode.
- Turbo gate keeps `work_gt_16667=0`.
- Preview cache/memory remains bounded.

### P1 - Workerize Launch Handoff

Move blocking launch prep and Main/FIFO wait into a launch worker.

Acceptance:

- Loading UI remains responsive until handoff succeeds or fails.
- Launch failure can redraw/recover without a frozen UI window.
- No direct Slint/core launch path is introduced.

### P1 - Reduce Cold Catalog Build Time

Target the cold first-scan path:

- directory traversal and metadata reads,
- MAME/software metadata load,
- SQLite import/materialized views.

Acceptance:

- `profile-first-scan.sh` consistently reports `library_ready < 45s` on the
  current SD card and catalog.
- First frame remains under 500ms.

### P2 - Split Warm Startup Catalog Hydration

Load compact Home/system state first, then hydrate full game rows in the
background.

Acceptance:

- Warm boot can interact with Home without waiting for all game rows.
- Arcade screen waits only when a game list is actually needed.
- Existing delayed stamp-check semantics remain intact.

### P2 - Throttle Background Media Work

Make media download/save idle-aware.

Acceptance:

- Media worker pauses or defers during active Arcade scroll and launch handoff.
- Device download concurrency defaults to one unless benchmarks prove higher is
  better.
- No media work before first visible launcher frame.

### P2 - Prewarm Lazy Rendering Tables

Audit fade/blend/table initialization and warm production tables after first
visible frame or before entering Arcade.

Acceptance:

- First preview fade does not pay table initialization in a traced UI frame.
- No measurable first-frame regression.

## Non-Recommendations

- Do not parallelize `/dev/fb0` writes. The current cached-RAM plus dirty-copy
  model is working, and present time is not the leading bottleneck.
- Do not reintroduce runtime PNG/JPG fallback for previews. Archive-backed
  raw565 loading is doing the right SD-card thing.
- Do not make catalog refresh automatic on a warm changed stamp. The current
  user-visible changed-library flow protects startup and scroll performance.
- Do not use experimental effect-scene results for production performance
  decisions.

## Flamegraph Follow-up

The earlier CPU flamegraph failure was a deploy-path failure, not a profile
runtime failure.

What failed:

- `scripts/profile-preview-scroll.sh --cpu-profile` built the ARM profile binary
  successfully.
- The resulting binary was 68,280,164 bytes (`65M`) versus 6,012,660 bytes for
  the normal production binary.
- Deploying that profile binary through `scripts/mister agent deploy-magik-bin`
  failed repeatedly with `Broken pipe (os error 32)`.
- The MiSTer had enough `/media/fat`, `/tmp`, and RAM headroom, and the agent
  was alive. No partial upload was left behind.
- Deploying the same profile binary through the non-agent SFTP path,
  `scripts/mister deploy-magik-bin`, succeeded in 16.2s with matching
  `local_bytes=68280164 remote_bytes=68280164`.

Conclusion:

- The flamegraph broke because the agent streaming deploy path is brittle for
  the large profiling binary.
- The production binary, SD card, target path, and profile runtime were not the
  root cause.
- `profile-preview-scroll.sh --cpu-profile` should either honor a deploy
  transport override, automatically fall back to the SFTP path for large profile
  binaries, or support a skip-deploy mode when the profile binary is already on
  the device.

Recovered artifacts:

- `build/preview-scroll-profiles/PERFAUDIT-20260624-CPU-RETRY-arcade-cpu.svg`
  (`302K`)
- `build/preview-scroll-profiles/PERFAUDIT-20260624-CPU-RETRY-arcade.tsv`
  (`434K`)
- `build/preview-scroll-profiles/PERFAUDIT-20260624-CPU-RETRY-arcade.log`
  (`512K`)

Profile run:

- Scenario: Arcade preview held-scroll, 60 seconds.
- Profile log:
  - `cpu_profile: sampling at 99 Hz`
  - `cpu_profile: 301 unique stacks, 2009 sample hits, 60.4s at 99 Hz`
  - `cpu_profile: wrote flamegraph ... (309034 bytes)`

Trace summary:

```text
frames=3599
prepare_us       p50=138   p95=579    p99=651    max=5375
slint_render_us  p50=166   p95=208    p99=317    max=370
custom_draw_us   p50=4145  p95=11781  p99=11889  max=12170
vsync_us         p50=10872 p95=15664  p99=16173  max=18178
fb_present_us    p50=939   p95=1043   p99=1123   max=1349
cached_present   p50=399   p95=502    p99=563    max=616
overlay_present  p50=505   p95=579    p99=640    max=841
wall_us          p50=16341 p95=16480  p99=16898  max=18325
```

Flamegraph hotspots:

- Preview transition/composition dominates CPU time during this scenario:
  - `blit_raw_preview_if_needed`: 1,166 samples, 58.04%.
  - `UiFrameTarget::blit_raw_preview_transition`: 1,161 samples, 57.79%.
  - `blit_transition_565_fade`: 1,161 samples, 57.79%.
  - `blit_transition_565_fade_generic`: 1,064 samples, 52.96%.
  - `blend_565`: 502 samples, 24.99%.
  - `raw565_row_pixel_or`: 339 samples, 16.87%.
- Same-geometry transition optimization is active but only covers a smaller
  share in this trace:
  - `blit_transition_565_fade_same_geometry`: 89 samples, 4.43%.
  - `blend_565_row`: 89 samples, 4.43%.
- Trace logging is visible in the CPU profile and should be discounted when
  interpreting production cost:
  - `LauncherFrameAccounting::finish_frame`: 118 samples, 5.87%.
  - `write_preview_trace` / formatted writes: 113 samples, 5.62%.
  - `__write`: 44 samples, 2.19%.
- Preview archive loading is secondary:
  - `preview-loader`: 135 samples, 6.72%.
  - `load_preview`: 71 samples, 3.53%.
  - `load_raw565_preview_asset_timed`: 53 samples, 2.64%.
  - `PreviewArchive::load_timed`: 48 samples, 2.39%.
  - LZ4 decode: 48 samples, 2.39%.
- Arcade row rendering is not the primary CPU bottleneck in this trace:
  - `ArcadeListRenderer::draw`: 42 samples, 2.09%.
  - `draw_content_band`: 31 samples, 1.54%.
  - `blit_cached_row_to_surface`: 27 samples, 1.34%.

Updated optimization reading:

- The highest-leverage production target is still preview fade/composition in
  RGB565, especially the generic geometry path and per-pixel blend helpers.
- Because trace logging accounts for roughly 5-6% of samples in this run,
  benchmark trace output should be treated as instrumentation overhead, not as a
  product hot path.
- Preview loading from raw565/LZ4 archive is visible but much smaller than the
  active fade/composition path during held scroll.
- The launcher was restored to the normal production binary after data
  collection. `scripts/mister status` showed RGB565 framebuffer mode, launcher
  on Home, and `mister-magik-fb` running normally.

## Gaps

- CPU flamegraph is now collected for the Arcade held-scroll preview scenario.
  Remaining gap: fix or bypass the agent large-binary deploy failure so
  `profile-preview-scroll.sh --cpu-profile` is one-command reliable again.
- Two delegated static review agents did not return before the hardware phase
  finished. The runtime/handoff delegated review completed and is reflected
  above; rendering and storage conclusions are based on direct code inspection
  plus hardware evidence.
- `profile-library-io.sh` did not capture useful I/O samples because it still
  targets the removed `library-scan-bench` command.
