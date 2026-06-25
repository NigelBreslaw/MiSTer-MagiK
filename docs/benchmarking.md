# Benchmarking And Profiling

This document defines current benchmark policy. Dated measurement logs live in
`history/`; use them as evidence, not as the command surface.

## General Rules

- Use RGB565 for production launcher and arcade conclusions. The UI/app
  benchmark path is RGB565-only; wider-color env overrides and color-route
  smoke paths are deleted from the app.
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

## Arcade And Preview Scenarios

Approved arcade scroll scenarios:

- `held-scroll` - normal continuous movement.
- `turbo-hold` - fast synthetic movement that reverses at list edges.
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
scripts/profile-arcade-scroll.sh LABEL
scripts/profile-preview-scroll.sh LABEL
```

For the "perfect 60fps Arcade preview" work, each single-commit PR must record
before/after device evidence with the same command set. Use labels that include
the PR slice and BEFORE/AFTER state:

```bash
scripts/profile-preview-scroll.sh 60 held-scroll LABEL-FADE-VEL --skip-build --visual-captures 0
scripts/profile-preview-scroll.sh 60 turbo-hold LABEL-FADE-TURBO --skip-build --visual-captures 0
scripts/profile-preview-scroll.sh 60 held-scroll LABEL-CPU-FADE-VEL --cpu-profile --visual-captures 0
```

The CPU profile command builds/deploys the profiling binary, runs the real
Main-supervised Arcade screen with `MISTER_PPROF=1`, exits after the trace
window so the profiler can flush, and pulls
`build/preview-scroll-profiles/LABEL-CPU-FADE-VEL-arcade-cpu.svg`.

Preview-scroll benchmarks synchronously warm the screenshot archive cache before
the benchmark timing window and first launcher step. Production preview evidence
uses the built-in 200ms fade; transition selection flags were removed from the
release benchmark script. `mega` transition coverage is experimental only and is
not release benchmark evidence.
`turbo-hold` ping-pongs between the Arcade list edges so long traces keep
exercising preview selection changes after reaching the bottom.

Acceptance fields for Arcade preview pacing:

- `work_gt_16_7ms` after frame 30 is reported as an outlier count.
- `vsync_source_fallback=0`, `vsync_source_timeout=0`,
  `vsync_source_error=0`, and `max_vsync_miss_streak=0`.
- `p99_work_us < 14500` for the preservation-of-fade milestone.
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

The gate runs 60s held-scroll fade and 60s turbo-hold fade, then fails if either
trace has non-vsync pacing sources, non-zero max miss streak, or p99 work
at/above the configured threshold. It reports `work_gt_16_7ms` separately so
isolated scheduler/prepare-wall outliers can be investigated without hiding p99
headroom. Pass `--baseline-label BASE` when validating a before/after change so
the gate also fails on present-path regressions in the copied RGB565 rows. Its
parser self-test is:

```bash
scripts/gate-preview-60fps.sh --self-test
```

These scripts write `/media/fat/mister-magik/launcher.env`, send
`mister_magik_restart_launcher`, and lock the real launcher on Arcade with:

```text
MISTER_LAUNCHER_START_SCREEN=arcade
MISTER_LAUNCHER_LOCK_SCREEN=arcade
MISTER_LAUNCHER_BENCH_SCENARIO=held-scroll|turbo-hold|preview-step-hold|idle
MISTER_PREVIEW_SCROLL_TRACE_SECS=N
MISTER_CATALOG_REFRESH=default
```

Arcade benchmark scripts use `MISTER_CATALOG_REFRESH=default`, not `off`.
Warm catalog startup may first populate Home/system counts from
`library.summary.json`; the default policy then hydrates the full SQLite catalog
without forcing a rebuild when the stamp matches. `off` leaves the launcher in
summary-only mode after a warm summary load and is invalid for Arcade row,
preview, and launch-handoff benchmarks because there may be no hydrated game
rows to scroll or launch. Set `on` or `force` only when intentionally
benchmarking a catalog rebuild.

Preview transition policy:

- Default real-app preview transition is fixed 200ms `fade`.
- Add new transition experiments under `scripts/experiments/preview/` and
  experiment builds rather than replacing the production `fade` effect.
- For visual review, use `MISTER_LAUNCHER_BENCH_SCENARIO=preview-step-hold`.

Historical evidence:

- `history/2026-6-8/arcade-band-copy-trial.md`
- `history/2026-6-11/rgb565-raw-preview-bench.md`
- `history/2026-6-13/preview-zstd-archive-bench.md`
- `history/2026-6-14/arcade-preview-identity-regression.md`

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
the sibling `../magik-cloud` repo with:

```bash
scripts/build-arcade-screenshot-pack.sh
scripts/build-neogeo-screenshot-pack.sh
scripts/build-console-screenshot-pack.sh --system saturn --input data/sources/saturn/canonical
```

`magik-cloud` writes resized PNGs, `.rgb565` files, and compressed LZ4 block
archives into ignored local artifact roots. Runtime preview loading uses the
archive path and asset key projected by the catalog; it must not derive cache
paths from PNG/JPG screenshot locations.

The preview loader reads each configured archive into memory when it opens the
archive. There is no runtime fallback to PNG/JPG sources, individual `.rgb565`
files, or per-entry archive file reads. The arcade pack measured on the MiSTer
at 34,623,433 bytes takes about 1.75s to cold-read from `/media/fat` into RAM
and about 0.24s once the filesystem cache is warm.

The library scanner must not walk screenshot/cache media directories, read
`gamelist.xml`, or probe normal PNG/JPG screenshots for metadata.

Runtime screenshot-pack downloads are selective: the catalog scan announces the
first discovered supported system, and the media worker checks/downloads only
those packs. Cached-catalog boots seed the same selective requests from the
ready catalog's installed systems after the first visible frame and after active
Arcade/launch interaction settles, so deleting packs without changing the
catalog still re-checks needed packs. Production download concurrency defaults
to one to avoid stealing SD-card headroom from interaction; diagnostic runs may
override `MISTER_MEDIA_CONCURRENCY`, clamped to the supported range. The
catalog-build screen is sourced from structured download/save progress events
rather than parsed log text.

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
render/timing failures are distinguishable from framebuffer capture failures.
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

`profile-first-scan.sh` deletes the production catalog database plus
`library.summary.json` and reboots with `scripts/mister reboot-wait`, which uses
the supervised `mister_magik_reboot` path when the Main fork is available. It
records first-frame/catalog-ready timings in
`history/toolchain-bench/results-first-scan.tsv`. For cold catalog UX, prefer
`bootstrap_counter_sustained_climb` over the first
`bootstrap_counter_climb`: the latter is only the first meaningful target
(`Games found: 50`), while the sustained metric marks the point where enough
real bootstrap count has reached the UI to keep the visible counter moving.
`full_scan_counter_climb` should mean the classifier count has overtaken the
currently displayed bootstrap count, not merely that classification reported its
first small batch.

`device-catalog-destruction.sh` is the manual recovery integration check for
missing, empty, corrupt, and marker-forced catalog states. Its missing-DB case
intentionally leaves any orphan `library.summary.json` in place and asserts the
launcher ignores that summary before showing the visible first-run scan; empty
and corrupt DB cases assert the same summary rejection for unusable SQLite
files.

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
scripts/profile-launch-prep.sh LABEL --replace-label --iterations 10
```

`profile-launch-handoff.sh` writes
`history/toolchain-bench/results-launch-handoff.tsv` rows with:

```text
label	iteration	launch_action_to_loading_us	max_frame_gap_us	loading_frames_before_result	failure_recovery_us	prepare_us	handoff_us	result
```

The target metric is launcher responsiveness during the blocking handoff path:
`max_frame_gap_us` and `failure_recovery_us` should improve or remain within the
existing frame budget while `profile-launch-prep.sh` p95 does not regress.
