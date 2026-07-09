# Benchmarking And Profiling

This document defines current benchmark policy. Dated measurement logs live in
`history/`; use them as evidence, not as the command surface.

## General Rules

- Use RGB565 for production launcher and arcade conclusions. The UI/app
  benchmark path is RGB565-only; wider-color env overrides and color-route
  smoke paths are deleted from the app.
- For each performance-changing commit, run the targeted "before" benchmark on
  the previous implementation, make one logical change, then rerun the same
  command shape after validation. Commit messages and evidence notes should name
  the labels and the metric that changed.
- Prefer short targeted benchmarks over broad soak runs. Scroll-path changes use
  one 30s `turbo-hold` run unless the code path needs a different scenario.
  Launcher pacing and frame-timing refactors use one 30s `human-turbo-hold` run
  because it exercises human-like bursts, pauses, and reversals while preserving
  the real Arcade entry flow.
  Avoid duplicate long scroll and turbo-scroll runs for the same claim.
- "Still 60fps" is not evidence by itself. Report the metric owned by the
  change: for example preview decode latency, catalog apply time/backlog,
  launch-prep p50/p95, `arcade_list_update_us`, or framebuffer present p95/p99.
- Before committing a performance change, run code review against the exact diff
  and tidy any findings before the final validation/benchmark rerun.
- Start visual benchmarks from a clean display-owner state. If stock OSD/menu is
  visible over the benchmark, the run is invalid even if the framebuffer PNG
  looks correct.
- Beware contaminated 30fps/vsync cadence after repeated restarts or immediate
  post-deploy runs. Settle, reboot, or rerun before declaring regressions.
- Do not compare short runs whose first seconds show `fps ~ 30` unless that is
  the behavior under test.
- Arcade benchmarks must run through `MiSTer_MagiK` supervising
  `mister-magik-fb ui launcher 0`. The removed direct Arcade scene is invalid
  for current performance conclusions because it bypasses Main's OSD, VT, and
  input ownership setup.

## Binary Scope Labels

Benchmark `run_context_tsv` rows identify the binary artifact that was expected
for the run:

- `binary_scope=prod-all`: production `release-device` build with
  `ui_scope=all`, suitable for whole-app release checks and general scene
  benchmarks.
- `binary_scope=launcher-scope`: production `release-device` build with
  `ui_scope=launcher`, suitable for launcher/Arcade production scroll evidence.
- `binary_scope=profile-launcher-scope`: profiling `release-device-profile`
  build with `ui_scope=launcher` and `features=ui,profile`, suitable only for
  CPU profile artifacts.
- `binary_scope=deployed-unknown`: `--skip-build` run where the script did not
  deploy or fingerprint the binary currently on the MiSTer. The row still
  records the expected local `profile`, `features`, path, and size, but runtime
  comparisons must account for possible stale profiling or alternate binaries.

Do not compare these as if they were the same artifact. A CPU-profile run must
be read as profiling evidence, not production frame-time evidence, and the
production `release-device` binary should be redeployed after any profiling
binary has been installed on the MiSTer.

## First Scan Gate

The cold first-scan gate measures time to a usable RAM catalog and time to a
durable SQLite save:

```bash
scripts/profile-first-scan.sh LABEL --skip-build --replace-label --thread-sample
```

Current production targets on the reference MiSTer are:

- `library_ready <= 57094ms`
- `library_db_saved <= 72573ms`

During first database creation, the catalog builder owns the machine. The
catalog worker and library walker must run foreground, with nice `0` and
unrestricted CPU affinity, until the RAM catalog is ready. Active screenshot
pack downloads also run at nice `0` with unrestricted CPU affinity because the
streaming thread, `curl`, and SHA-256 verifier directly drive a visible progress
bar. Do not apply the background CPU0/nice policy to these first-build scan or
visible media-download paths. Dropped frames or less-smooth scan-screen
animation are acceptable during this window because the launcher has no usable
catalog yet; failing the readiness gate is not.

The low-priority CPU0 policy remains appropriate for warm validation, preview
prefetch, media coordination, index sidecar repair, and other background jobs
after a usable catalog exists.
Use `--thread-sample` when changing catalog scheduling so the run proves the
first-build roles are foreground and the later background roles remain isolated.

## Startup Reveal Gate

Startup reveal checks cover the three launcher entry modes: cold no-catalog,
warm valid-catalog, and return-from-game. Run the acceptance script against a
freshly deployed production binary when changing launcher lifecycle, catalog
startup, preview readiness, or launch-return behavior:

```bash
scripts/device-startup-reveal-acceptance.sh LABEL
```

For the broader hardware-in-the-loop (HIL) release policy, tier semantics, skip
reporting, and artifact contract, see `docs/production-readiness.md`.

The script backs up and removes the device catalog for the cold scenario, then
restores it before the warm and return scenarios. It appends
`history/toolchain-bench/results-startup-reveal.tsv` rows with `mode`,
`reveal_ms`, `input_enabled_ms`, `catalog_ready_ms`, `first_frame_ms`,
`preview_state`, and `result`. Generated TSV rows are measurement evidence and
should not be committed unless a release note or investigation explicitly needs
the captured device run.

Acceptance depends on `/tmp/mister-magik/status.json` reporting
`startup_mode`, `startup_reveal_state`, `revealed`, `input_enabled`,
`reveal_ms`, and `input_enabled_ms`, plus startup timing rows in
`/tmp/mister-magik/events.jsonl` and the launcher log. Warm boots must not emit
`startup_splash_visible`; return-from-game must restore Arcade selection before
`launcher_revealed` and wait for `return_preview_ready`.

## Home System Row Scenarios

Use `home-repeat-hold` when measuring the experience of holding left or right
on the Home system row. The scenario feeds held d-pad input through the normal
launcher input path, so it includes the real repeat behavior: immediate first
move, 1s initial repeat delay, then 80ms repeat. At either end of the system
list it reverses direction and repeats, which keeps long traces exercising both
left and right movement.

```bash
scripts/bench-toolchain.sh LABEL --replace-label --device --scene-secs 30 --launcher-scenario home-repeat-hold --ui-scope launcher
```

Use the strict zero-drop gate when the symptom is visible missed frames while
holding left/right across the Home system row:

```bash
scripts/gate-launcher-home-max-scroll-zero-drops.sh LABEL --secs 30 --skip-build
```

The default gate follows the production renderer,
`fpga-vblank-latch-hidden`, and collects passive `fpga-latch-report` samples
before and after the run. To force the legacy fallback path for comparison, pass
`--present-backend fb0-dirty`:

```bash
scripts/gate-launcher-home-max-scroll-zero-drops.sh LABEL --secs 30 --skip-build --present-backend fb0-dirty
```

The gate writes `build/launcher-home-scroll-profiles/*-launcher-home-scroll.tsv`
and a matching `*-launcher-home-scroll-drops.tsv` report. The `/dev/fb0`
fallback gate treats `wall_us > 16667` or `loop_delta_us > 16667` as visual
cadence failure because userspace copies to the scanned framebuffer after
vblank. The FPGA latch gate uses latch-visible evidence instead: every measured
frame must use the latch backend with status `ok`, post before the latch deadline,
alternate hidden buffers, keep sampled FPGA flip counters consistent when they
are present, and finish with passive `fpga-latch-report drop_count=0`.
`wall_us` and `loop_delta_us` remain in the latch report as
`scheduler_wake_jitter_misses`, but they are not latch visual misses by
themselves because the FPGA consumes the already-posted hidden buffer at vblank.
The report also includes latch copy/post/status timings, latch deadline margin,
and finalization timing (`frame_finish_us` plus `post_finish_tail_us`). In latch
mode, benchmark trace rows are buffered during the hot path so periodic TSV
flushes do not masquerade as TV-visible frame skips.

`drop_count=0` from passive `fpga-latch-report` proves that the FPGA accepted
the posted buffers. Combined with zero latch deadline misses, alternating
buffers, and consistent sampled flip-counter deltas, it is the latch visual
smoothness signal. Use passive `fpga-latch-report` for before/after FPGA counters;
`fpga-latch-post-report` posts a diagnostic latch request and can change the
counters it reports. The shared trace schema predates the Home-row gate, so the
`selected` and `visual_index` columns still describe the Arcade list, not the
Home system index; use the log/status
`bench_scenario=home-repeat-hold` fields to confirm the Home benchmark path. The
default `MISTER_CATALOG_REFRESH=off` isolates Home-row pacing from catalog
refresh noise; pass `--catalog-refresh default` when deliberately measuring the
normal startup mix.

Use `home-nav` only for synthetic fixed-period Home-row stepping; it does not
model the real d-pad repeat gate.

## Arcade And Preview Scenarios

Approved arcade scroll scenarios:

- `held-scroll` - normal continuous movement.
- `turbo-hold` - fast synthetic movement that reverses at list edges.
- `human-turbo-hold` - bursty human-like turbo movement with short pauses; use
  this as the pacing regression gate for launcher frame-timing refactors.
- `velocity-scroll` - alias for `held-scroll`.

Deprecated for arcade performance conclusions:

- `list-scroll`
- old `smooth-scroll`
- manual selected-index jumps
- row-by-row or stepwise scenarios
- the old live-framebuffer scroll-present path
  (`MISTER_ARCADE_SCROLL_PRESENT` / `--scroll-present`)

Use these entrypoints:

```bash
scripts/profile-arcade-scroll.sh LABEL --secs 30 --scenario turbo-hold
scripts/profile-preview-scroll.sh LABEL --secs 30 --scenario turbo-hold
scripts/profile-first-preview.sh LABEL --skip-build
```

`profile-arcade-scroll.sh` is the reproduction gate for boot-entry stutter. Its
default path reboots the MiSTer, starts the launcher on Home, quickly navigates
to Arcade using `MISTER_ARCADE_ENTRY_INPUT_SCRIPT` or the default Right...A
sequence, then starts the timed `turbo-hold` trace in that same launcher
session. Use `--skip-boot-prelude` only for old direct-to-Arcade comparisons;
do not use that shortcut as evidence for the user-visible boot-to-Arcade flow.
`human-turbo-hold` uses the same Main-supervised Arcade entry path and requires
a bench-tools MagiK binary, so use `--deploy-device` when collecting pacing
regression evidence from a fresh commit.
The script emits and enforces `frame_pacing_gate_tsv` for the 60fps/drop-frame
contract and `preview_exact_gate_tsv` for the no-skipped-preview contract.
For `human-turbo-hold`, the pacing gate treats small wall-time jitter above one
60 Hz period as diagnostic rather than failing evidence because the scenario
intentionally mixes bursts, pauses, reversals, and real entry flow. It still
hard-fails any work frame over budget, any wall frame over 33 ms, any
fallback/timeout/error/unknown vsync source, and any non-zero max miss streak.
For other arcade scenarios, the strict wall gate remains unchanged.
The turbo preview runway defaults to 32 previews ahead; use
`MISTER_PREVIEW_TURBO_LOOKAHEAD=64` to reproduce the old aggressive prefetch
behavior, or `MISTER_PREVIEW_TURBO_RUNWAY=0` only as a diagnostic because it
allows stale/empty previews during turbo scroll.

For the "perfect 60fps Arcade preview" work, each single-commit PR must record
before/after device evidence with the same command set. Use labels that include
the PR slice and BEFORE/AFTER state:

```bash
scripts/profile-preview-scroll.sh LABEL-FADE-TURBO --skip-build --secs 30 --scenario turbo-hold --visual-captures 0
scripts/profile-preview-scroll.sh LABEL-CPU-FADE-TURBO --cpu-profile --secs 30 --scenario turbo-hold --visual-captures 0
```

The CPU profile command builds/deploys the profiling binary, runs the real
Main-supervised Arcade screen with `MISTER_PPROF=1`, exits after the trace
window so the profiler can flush, and pulls
`build/preview-scroll-profiles/LABEL-CPU-FADE-TURBO-arcade-cpu.svg`.
Its `run_context_tsv` row is marked `runtime_type=profile`,
`binary_scope=profile-launcher-scope`, and `production_restore_required=1`.

Preview-scroll benchmarks synchronously warm the screenshot archive cache before
the benchmark timing window and first launcher step unless
`--skip-preview-warm` is passed. Use warm runs for steady 60fps preview evidence
and cold no-warm runs for screenshot-pack index fast-lane work. Production
preview evidence uses the built-in 200ms fade; transition selection flags were
removed from the release benchmark script. `mega` transition coverage is
experimental only and is not release benchmark evidence.
Production preview composition presents the raw preview layer directly by
default. Set `MISTER_PREVIEW_DIRECT_PRESENT=0` only for cached-path A/B
measurements.
`turbo-hold` ping-pongs between the Arcade list edges so long traces keep
exercising preview selection changes after reaching the bottom.

Acceptance fields for Arcade preview pacing:

- Screenshot previews must be exact or intentionally empty for every sampled
  frame in the benchmark trace. `cache_state` values other than `exact` or
  `empty` are failures, even when frame pacing remains clean.
- The trace must include active production fade samples
  (`transition_effect=fade` with `0 < transition_progress < 1`). A hard cut to
  the final preview is a failure, even when every sampled preview is exact.
- `work_gt_16_7ms` after frame 30 is reported as an outlier count.
- `vsync_source_fallback=0`, `vsync_source_timeout=0`,
  `vsync_source_error=0`, and `max_vsync_miss_streak=0`.
- `p99_work_us < 14500` for the preservation-of-fade milestone.
- `profile-arcade-scroll.sh` hard-fails this contract through
  `frame_pacing_gate_tsv`; the p99 work threshold can be overridden with
  `MISTER_ARCADE_SCROLL_P99_WORK_US` for diagnostic comparisons only.
- For render-contract, framebuffer-format, route, or copy-helper changes, use
  `scripts/launcher-present-trace.py compare BEFORE.tsv AFTER.tsv` and report
  the `present_path_tsv` rows. `cached_present_us`, `arcade_list_present_us`,
  and `fb_present_us` p95/p99 must stay within +5%, and `rows` p95/p99 must not
  increase by more than one row.
- Visual captures must preserve the current fade appearance when enabled.

After the preview fade optimization work, run the final release gate with:

```bash
scripts/gate-preview-60fps.sh LABEL --skip-build --visual-captures 0
```

The gate is for release-candidate confirmation. Per-commit scroll evidence uses
the shorter targeted 30s `turbo-hold` profile above, then the gate can be run
with `--secs 30` when a combined preservation check is useful. It fails if a
trace has non-vsync pacing sources, non-zero max miss streak, any non-exact
preview cache state, or p99 work at/above the configured threshold. It reports
`work_gt_16_7ms` separately so isolated scheduler/prepare-wall outliers can be
investigated without hiding p99 headroom. Pass `--baseline-label BASE` when
validating a before/after change so the gate also fails on present-path
regressions in the copied RGB565 rows. Its parser self-test is:

```bash
scripts/gate-preview-60fps.sh --self-test
```

These scripts write `/media/fat/mister-magik/launcher.env`, send
`mister_magik_restart_launcher`, and lock the real launcher on Arcade with:

```text
MISTER_LAUNCHER_START_SCREEN=arcade
MISTER_LAUNCHER_LOCK_SCREEN=arcade
MISTER_LAUNCHER_BENCH_SCENARIO=held-scroll|turbo-hold|preview-step-hold|preview-idle|idle
MISTER_PREVIEW_SCROLL_TRACE_SECS=N
MISTER_PREVIEW_SCROLL_SKIP_ARCHIVE_WARM=1  # only for cold fast-lane benchmarks
MISTER_CATALOG_REFRESH=default
```

Launcher velocity scenarios and preview scroll TSVs require a MagiK binary
built with `--bench-tools`; production `ui` builds intentionally ignore
`MISTER_LAUNCHER_BENCH_SCENARIO` and omit trace writers.

Cold direct-to-system turbo preview readiness is measured with:

```bash
scripts/gate-cold-turbo-preview.sh LABEL --systems arcade,neogeo,saturn --secs 10
```

This gate reboots per system, starts the launcher directly with
`MISTER_LAUNCHER_START_SYSTEM=<system>`, skips full archive warming, enables the
64-item turbo preview runway, and runs `turbo-hold`. It fails if any turbo
selection sample with a preview-capable candidate is blank, stale, failed, or
shows another asset key. Passing rows report `miss_count=0`, `blank=0`,
`stale=0`, `archive_mem_loads=0`, and first selected loads from `index_pread`.
Use `scripts/profile-cold-turbo-preview.sh` without `--require-pass` when
collecting reporting-only data.

Arcade benchmark scripts use `MISTER_CATALOG_REFRESH=default`, not `off`.
Warm catalog startup may first populate Home/system counts from
`library.summary.json`; the default policy then hydrates the full SQLite catalog
without forcing a rebuild when the stamp matches. `off` leaves the launcher in
summary-only mode after a warm summary load and is invalid for Arcade row,
preview, and launch-handoff benchmarks because there may be no hydrated game
rows to scroll or launch. Set `on` or `force` only when intentionally
benchmarking a catalog rebuild.

Warm validation includes both the root stamp and the discovery checkpoint. The
unchanged path must stay under the existing 2s hard gate including checkpoint
load, compute, compare, drift classification, and worker decision. Coarse
`catalog_checkpoint_tsv` and `catalog_drift_tsv` rows are always emitted. Set
`MISTER_CATALOG_TRACE=detail` only for diagnostics that need per-core
`catalog_profile_manifest_tsv` rows.

Preview transition policy:

- Default real-app preview transition is fixed 200ms `fade`.
- Add new transition experiments under `scripts/experiments/preview/` and
  experiment builds rather than replacing the production `fade` effect.
- For visual review, use `MISTER_LAUNCHER_BENCH_SCENARIO=preview-step-hold`.
- For first selected screenshot latency, use `scripts/profile-first-preview.sh`;
  it runs `preview-idle` so the initial selected result can apply without being
  superseded by a scroll step.

For screenshot pack codec experiments, recode the installed/device-derived
packs. Do not build from the full source screenshot pool unless the experiment
explicitly needs a publish-sized corpus; arcade decode conclusions should use
the same roughly 800-900 entries visible to MiSTer MagiK on the device.

```bash
scripts/magik-cloud run -- cargo run --quiet -- pack-recode \
  --variant mmlz4b-v2-lz4-hc-9-pixels \
  --input /ABS/PATH/device-packs/arcade-screenshots-320x320.mmlz4b \
  --output /ABS/PATH/variants/mmlz4b-v2-lz4-hc-9-pixels/arcade-screenshots-320x320.mmlz4b
scripts/mister put \
  /ABS/PATH/variants/mmlz4b-v2-lz4-hc-9-pixels/arcade-screenshots-320x320.mmlz4b \
  /media/fat/mister-magik/bench/arcade-v2-hc9-pixels.mmlz4b
scripts/profile-preview-pack-decode.sh LABEL \
  --variant mmlz4b-v2-lz4-hc-9-pixels \
  --pack /media/fat/mister-magik/bench/arcade-v2-hc9-pixels.mmlz4b
scripts/profile-preview-scroll.sh 60 turbo-hold LABEL --skip-build --visual-captures 0
```

The decode benchmark reports per-entry `decode_us`, `decode_cpu_us`,
`raw565_parse_us`, `raw565_parse_cpu_us`, `total_us`, `encoded_bytes`,
`decoded_bytes`, and pack-size rows for arcade, Neo Geo, and Saturn when the
matching `--arcade-pack`, `--saturn-pack`, and `--neogeo-pack` paths are
supplied. `decode_us` is monotonic wall time around only the decompressor call;
`decode_cpu_us` is Linux thread CPU time around the same call. Treat large
wall-only outliers as scheduler/device noise unless the CPU column moves with
them. The turbo launcher benchmark emits `warm_gate_tsv`; codec evidence is
invalid unless `loaded=1` and `valid=1` show that the full screenshot archive
was warmed before the 60 second timing window.

Historical evidence:

- `history/2026-6-8/arcade-band-copy-trial.md`
- `history/2026-6-11/rgb565-raw-preview-bench.md`
- `history/2026-6-13/preview-zstd-archive-bench.md`
- `history/2026-6-14/arcade-preview-identity-regression.md`
- `history/2026-6-26-screenshot-codec-bench.md`

## Preview Cache Policy

Original arcade screenshots live on the MiSTer under:

```text
/media/fat/_Arcade/media/screenshot
```

Generated MagiK screenshot packs live under:

```text
/media/fat/mister-magik/assets
```

Only generated cache directories should be deleted/recreated. Runtime preview
loading is raw565-oriented; build caches and publish-ready packs from the Mac in
the private `private/magik-cloud` submodule with:

```bash
scripts/magik-cloud run -- scripts/build-arcade-screenshot-pack.sh --launcher-db /ABS/PATH/library.sqlite3
scripts/magik-cloud run -- scripts/build-neogeo-screenshot-pack.sh
scripts/magik-cloud run -- scripts/build-console-screenshot-pack.sh --system saturn --input data/sources/saturn/canonical
```

The Arcade pack must be built from the deployed launcher catalog keys. Do not
publish `--all-mame-families` output; that mode is only for diagnostic
full-source experiments and does not match the normal MiSTer MagiK arcade list.

`magik-cloud` writes resized PNGs, `.rgb565` files, and production
`mmlz4b-v2-lz4-hc-9-pixels` archives into ignored local artifact roots. Runtime
preview loading uses the archive path and asset key projected by the catalog; it
must not derive cache paths from PNG/JPG screenshot locations.

The preview loader reads each configured v2 pixel archive into memory when it
opens the archive. Production screenshot packs also publish `.mmlz4b.idx`
sidecars so cold selected and prefetch preview requests can `index_pread` one
payload and show the first screenshot before full-pack warm finishes. Once the
full pack is loaded, steady-state requests return to `archive_mem`.
There is no runtime fallback to PNG/JPG sources or individual `.rgb565` files.
The arcade pack measured on the MiSTer at 34,623,433 bytes takes multiple
seconds to cold-read from `/media/fat` into RAM, so first-preview claims must
state whether `load_source` was `index_pread` or `archive_mem`.

The library scanner must not walk screenshot/cache media directories, read
`gamelist.xml`, or probe normal PNG/JPG screenshots for metadata.

Runtime screenshot-pack downloads are selective: the catalog scan announces the
first discovered supported system, and the media worker checks/downloads only
those packs. Cached-catalog boots seed the same selective requests from the
ready catalog's installed systems after the first visible frame and after active
Arcade/launch interaction settles, so deleting packs without changing the
catalog still re-checks needed packs. Production runs one active pack download
at a time to avoid stealing network, CPU, and SD-card headroom from interaction;
the active pack may still fetch its small index sidecar in parallel. The
catalog-build screen is sourced from structured download/save progress events
rather than parsed log text. The visible download phase streams the raw pack to
`/tmp/mister-magik-media-download` and hashes those bytes there; the later
save/publish phase copies the verified staged file to `/media/fat` and performs
the atomic sync/rename.

Use `scripts/profile-screenshot-download.sh` to measure network download,
verify, save/publish, and total wall time:

```bash
scripts/profile-screenshot-download.sh MEDIA-DL-YYYYMMDD --system neogeo --iterations 1 --replace-label
```

The TSV output is:

```text
screenshot_download_bench_tsv	label	system	variant	encoded_bytes	decoded_bytes	download_ms	decompress_ms	save_ms	verify_ms	total_ms	wire_mbps	decoded_mbps	etag	content_encoding	cf_cache_status	result
```

Use `scripts/profile-screenshot-save.sh` to measure save-progress overhead
separately from network and checksum cost:

```bash
scripts/profile-screenshot-save.sh SAVE-PROGRESS-YYYYMMDD --system neogeo --iterations 10
```

The TSV output is:

```text
screenshot_save_bench_tsv	label	system	mode	iteration	bytes	copy_ms	sync_ms	rename_ms	parent_sync_ms	total_ms	progress_events	result
```

Compare average and p95 `total_ms` plus `copy_ms` when changing production save
behavior. Benchmark claims for screenshot media must state whether they cover
download, decompression, save/publish, verification, and total wall time.

When evaluating media work during Arcade interaction, also run a preview scroll
trace while media requests are pending. Use `frame_pacing` p95/p99 work,
`work_gt_16_7ms`, `preview_latency selected_*_age_us`, and RSS HWM from the log
or status rows. Do not use "still 60fps" as proof; the app can remain vsync
paced while losing CPU or SD-card headroom.

For screenshot-pack index work, run both:

```bash
scripts/profile-preview-index-refresh.sh PREVIEW-IDX-DB-YYYYMMDD
scripts/profile-first-preview.sh FIRST-IDX-YYYYMMDD --skip-build
scripts/profile-preview-scroll.sh SCROLL-IDX-YYYYMMDD --skip-build --secs 30 --scenario turbo-hold --skip-preview-warm --visual-captures 0
```

Acceptance evidence should show per-system DB refresh timing from
`preview_index_refresh_tsv`, the first selected decode using
`load_source=index_pread` in the sub-250ms target range, scrolling previews
visible before the full pack load completes, and later steady-state preview
decodes returning to `archive_mem`.

Relevant docs:

- `history/2026-6-13/arcade-screenshot-cache-workflow.md`
- `history/2026-6-14/library-scanner-preview-archive-pruning.md`

## Experiments

Effect-scene profiling and `mega` preview-transition runs are experiments, not
release benchmark evidence. Their scripts live under `scripts/experiments/`,
require an experiment-enabled binary, and are documented in
`docs/experiments/effects.md`.

## Toolchain And Scene Benchmarks

General scene and toolchain benchmark entrypoints:

```bash
scripts/bench-toolchain.sh LABEL --replace-label
mister-magik-fb scenes
mister-magik-fb ui <scene> <secs>
```

`scripts/bench-toolchain.sh` appends formal results to
`history/toolchain-bench/results.tsv`. The TSV keeps the legacy `visual_ok`
column as a combined pass bit and also records `timing_ok` and `capture_ok` so
render/timing failures are distinguishable from agent framebuffer capture
failures. Use `scripts/mister agent framebuffer-capture OUT.png --json
OUT.json` for ad hoc framebuffer PNGs; do not add raw `/dev/fb0` dump or
host-side raw-to-PNG capture paths. For live desktop inspection and FPS
experiments, use the producer-side framebuffer stream (`framebuffer_stream_v1`)
instead of repeated PNG or raw-frame polling; keep PNG capture for still
artifacts and capture validation.
Build profiles and toolchain details live in `magik-gui/BUILD.md`.

Bench scene documentation lives in `magik-gui/ui/bench/README.md`.

## Library Benchmarks

Use library benchmark scripts and SQL inspection rather than pulling the SQLite
database back to the host:

```bash
scripts/profile-first-scan.sh LABEL --deploy-device --replace-label
scripts/profile-library-save.sh LABEL --iterations 5 --replace-label
scripts/profile-library-io.sh LABEL --replace-label
scripts/bench-library.sh
scripts/mister db
scripts/mister db "SELECT count(*) FROM games"
```

Release-device builds expose the read-only `library-sql` command used by
`scripts/mister db`. Successful queries print normal result rows first and then
append a `library_sql_timing_tsv` row for SQLite open, prepare, first-row,
row-read, formatting, total query time, row count, column count, and result byte
count. If the wrapper says it is using the SFTP fallback, the timing describes a
host-side query against a copied database rather than direct device SQLite
performance.

`profile-first-scan.sh` deletes the production catalog database plus
`library.summary.json`, syncs, and reboots with
`scripts/mister reboot-wait --direct-reset` because no further writes are
expected before reset. It records first-frame/catalog-ready timings in
`history/toolchain-bench/results-first-scan.tsv`. The hard first-scan gates are
`library_ready <= 57094ms` for RAM catalog usability and
`library_db_saved <= 72573ms` for durable SQLite save completion. Anything above
either threshold fails the script. For cold catalog UX, prefer
`bootstrap_counter_sustained_climb` over the first
`bootstrap_counter_climb`: the latter is only the first meaningful target
(`Games found: 50`), while the sustained metric marks the point where enough
real bootstrap count has reached the UI to keep the visible counter moving.
`full_scan_counter_climb` should mean the classifier count has overtaken the
currently displayed bootstrap count, not merely that classification reported its
first small batch.
`counter_plateau` is derived as
`full_scan_counter_climb - bootstrap_counter_sustained_climb`; use it as the
first-scan "felt stuck" metric when changing bootstrap progress or scanner
progress reporting. `catalog_worker_ram_catalog` records the staged in-memory
catalog projection cost and must be reported separately from scan time and
SQLite save time.

For cold-scan retention decisions, judge scanner optimizations against
`library_scan_complete`, `scan_stage_walk`, `scan_stage_file_discovery`, and
`scan_stage_classify_total`. Do not count `library_db_saved`,
`import_stage_total`, SQLite publish, or saved-catalog hydration toward scanner
speedup claims. Non-UX scanner changes should save at least 8s on cold
`profile-first-scan.sh` runs against the relevant baseline before they earn
their complexity.

`device-catalog-destruction.sh` is the manual recovery integration check for
missing, empty, corrupt, and marker-forced catalog states. Its missing-DB case
intentionally leaves any orphan `library.summary.json` in place and asserts the
launcher ignores that summary before showing the visible first-run scan; empty
and corrupt DB cases assert the same summary rejection for unusable SQLite
files.

`device-catalog-drift-acceptance.sh` is the manual real-device check for
checkpoint drift. It covers missing-DB first creation, warm unchanged
validation, known core and game additions, unknown core audit drift, new system
directory drift, Continue marker deferral, marker rebuild, and immediate
Rebuild. Rows are appended to
`history/toolchain-bench/results-catalog-drift-acceptance.tsv`.

`bench-library.sh` suspends the supervised launcher through `/dev/MiSTer_cmd`
while running scanner/import CLI benchmarks. Do not benchmark by directly
killing `mister-magik-fb`; that can leave the Main fork and display/OSD state
out of sync.

`profile-library-io.sh` runs one scanner/import benchmark while sampling
process CPU ticks, process I/O bytes, system CPU/iowait, and SD-card diskstats
once per second. Use it before claiming that a scanner/import change is CPU- or
I/O-bound.

`profile-library-save.sh` runs fresh `library-refresh` passes against disposable
database paths and captures only the final `library_sqlite_publish_tsv` rows.
Use it when changing file publish behavior so save timing is separated from
catalog discovery and SQLite import work. `profile-first-scan.sh` also records
the publish row during full cold-start measurements so the final `library_ready`
time can be read alongside the save phase.

Set `MISTER_LIBRARY_BENCH_FORCE_REBUILD=1` only on disposable roots when
measuring explicit full-build refresh behavior; it creates a synthetic
candidate file.

Use `scripts/bench-library.sh LABEL --precount` only to measure the cost of a
pre-scan candidate count for determinate discovery progress. Use
`--sqlite-build-dir /tmp` only to benchmark the opt-in tmpfs SQLite build path.

## Warm Catalog Startup

Use the warm catalog startup script to measure summary-projection startup and
full SQLite hydration separately:

```bash
scripts/profile-warm-catalog-start.sh LABEL --replace-label --iterations 5
```

Rows are appended to `history/toolchain-bench/results-warm-catalog.tsv` with:

```text
label	iteration	first_frame_ms	first_frame_catalog_ready	catalog_cache_load_sync_ms	catalog_cache_load_sync_total_us	catalog_summary_load_ms	catalog_summary_load_us	catalog_bridge_systems_us	catalog_bridge_sync_us	full_catalog_ready_ms	full_catalog_ready_load_us	result
```

For warm-start claims, report first interactive Home/system-list time,
`catalog_summary_load_us`, whether `catalog_cache_load_sync` stayed off the
pre-loop path, first-frame time, and full catalog ready time.

## Launch Handoff

Use launch-handoff benchmarks when changing launch preparation, FIFO/Main
handoff, or launch failure recovery:

```bash
scripts/profile-launch-handoff.sh LABEL --replace-label --iterations 5
scripts/profile-launch-prep.sh LABEL-WARM --replace-label --scenario warm --iterations 5
scripts/profile-launch-prep.sh LABEL-COLD --replace-label --scenario cold --iterations 3
```

`profile-launch-handoff.sh` writes
`history/toolchain-bench/results-launch-handoff.tsv` rows with:

```text
label	iteration	launch_action_to_loading_us	max_frame_gap_us	loading_frames_before_result	failure_recovery_us	prepare_us	handoff_us	result
```

The target metric is launcher responsiveness during the blocking handoff path:
`max_frame_gap_us` and `failure_recovery_us` should improve or remain within the
existing frame budget while `profile-launch-prep.sh` p95 does not regress.
