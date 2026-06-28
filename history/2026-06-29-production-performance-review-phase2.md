# Production Performance Review Phase Two - 2026-06-29

Scope: production benchmarks on the real MiSTer at `192.168.1.117`.
Experimental effects were excluded. The benchmarked code revision was
`5d966944`; the working tree was dirty because this review added report files
and benchmark TSV rows.

After the CPU-profile run deployed a profiling binary, the normal production
`release-device` binary was rebuilt and redeployed. Final health check:

- `mister-magik-fb` running under `MiSTer_MagiK`.
- RGB565 framebuffer, `960x540`.
- Launcher at Home, 60fps.
- `SELECT count(*) FROM games` returned `9237`.

## Executive Summary

The production launcher is comfortably 60fps in steady Arcade/preview browsing.
The remaining high-value performance work is catalog/storage orchestration and
background scheduling, not raw framebuffer rendering.

Key results:

- Warm startup: first frame `22-28ms`, full catalog readiness about `2.35s`.
- Arcade turbo scroll: all-vsync pacing, no fallback/timeouts/errors, p99
  framebuffer present about `1.08ms`.
- Preview turbo scroll: p99 work `3.39ms`, zero work frames over `16.7ms`.
- Preview release gate: passed held-scroll and turbo-hold.
- First selected preview: `index_pread`, selected decode queue age `1.086ms`,
  selected decode total `21.077ms`, applied at `90.826ms`.
- First scan: first frame `73ms`, RAM catalog ready `40.282s`, DB saved
  `51.655s`.
- SQLite publish: `6.86MB` DB copy/publish about `0.51-0.67s`.
- Screenshot pack save: `4.97MB` Neo Geo pack save about `0.39-0.57s`.
- Launch prep: warm p50 `49us`, p95 `2.997ms`; cold p50 `22us`, p95
  `3.008ms`.

## Rendering And Scroll

### Arcade Turbo Scroll

Command:

```bash
scripts/profile-arcade-scroll.sh PERF20260629-ARCADE --secs 30 --scenario turbo-hold --skip-build
```

Artifact:

- `build/arcade-scroll-profiles/PERF20260629-ARCADE-arcade-scroll.tsv`

Summary:

- Frames after warmup: `1717`.
- Vsync: `1717`; fallback/timeout/error: `0`; max miss streak: `0`.
- `prepare_us`: p50 `220`, p95 `585`, p99 `644`.
- `custom_draw_us`: p50 `1333`, p95 `1609`, p99 `1691`.
- `fb_present_us`: p50 `948`, p95 `1035`, p99 `1080`.
- `cached_present_us`: p99 `517`.
- `arcade_list_present_us`: p99 `599`.
- Rows copied: p99 `704`.

Interpretation:

The scroll path is healthy. It still copies almost the whole arcade list overlay
each frame, but that copy is below 1.1ms p99. Dense-copy and copy-shape
experiments are worth testing, but this is not the current bottleneck.

### Preview Turbo Scroll

Command:

```bash
scripts/profile-preview-scroll.sh PERF20260629-PREVIEW --secs 30 --scenario turbo-hold --skip-build --visual-captures 0
```

Artifact:

- `build/preview-scroll-profiles/PERF20260629-PREVIEW-arcade.tsv`
- `build/preview-scroll-profiles/PERF20260629-PREVIEW-arcade-report.html`

Summary:

- Frames after warmup: `1717`.
- p99 work: `3386us`.
- Work frames over `16.7ms`: `0`.
- Vsync: `1717`; fallback/timeout/error: `0`; max miss streak: `0`.
- `custom_draw_us`: p99 `1695`.
- `preview_blit_us`: p99 `1645`.
- `arcade_list_present_us`: p99 `704`.
- Preview timing: `524` decode rows, `509 archive_mem`, `13 index_pread`,
  `1 decoded_cache`.
- No unexpected file reads and no slow reads.

Interpretation:

The preview render path is well within frame budget. The production fade and
raw565 blit are not the immediate problem.

### Preview No-Warm Scroll

Command:

```bash
scripts/profile-preview-scroll.sh PERF20260629-PREVIEW-COLD --secs 30 --scenario turbo-hold --skip-build --skip-preview-warm --visual-captures 0
```

Artifact:

- `build/preview-scroll-profiles/PERF20260629-PREVIEW-COLD-arcade.tsv`

Summary:

- Frames after warmup: `1750`.
- p99 work: `3151us`.
- Work frames over `16.7ms`: `0`.
- Vsync: `1750`; fallback/timeout/error: `0`; max miss streak: `0`.
- Preview timing: `535` decode rows, `520 archive_mem`, `13 index_pread`,
  `1 decoded_cache`.

Interpretation:

The index fast lane and background archive warm behave acceptably even when the
benchmark skips prewarming. It is still worth adding metadata TTL or
media-generation invalidation for preview archive stats, but no urgent frame
budget failure showed here.

### Preview Release Gate

Command:

```bash
scripts/gate-preview-60fps.sh PERF20260629-GATE --skip-build --visual-captures 0
```

Artifacts:

- `build/preview-scroll-profiles/PERF20260629-GATE-FADE-VEL-arcade.tsv`
- `build/preview-scroll-profiles/PERF20260629-GATE-FADE-TURBO-arcade.tsv`

Held-scroll summary:

- Frames after warmup: `3517`.
- p99 work: `3122us`.
- Work frames over `16.7ms`: `0`.
- Vsync: `3517`; fallback/timeout/error: `0`; max miss streak: `0`.

Turbo-hold summary:

- Frames after warmup: `3517`.
- p99 work: `3127us`.
- Work frames over `16.7ms`: `2`.
- Vsync: `3517`; fallback/timeout/error: `0`; max miss streak: `0`.
- The two over-budget work frames were attributed to dominant custom draw.

Interpretation:

The gate passed. The two turbo custom-draw outliers are small enough not to
threaten the gate, but they point at the same next target: reduce rare
`custom_draw_us` spikes under aggressive scroll.

### CPU Profile

Command:

```bash
scripts/profile-preview-scroll.sh PERF20260629-PREVIEW-CPU --secs 30 --scenario turbo-hold --skip-build --cpu-profile --visual-captures 0
```

Artifacts:

- `build/preview-scroll-profiles/PERF20260629-PREVIEW-CPU-arcade.tsv`
- `build/preview-scroll-profiles/PERF20260629-PREVIEW-CPU-arcade-cpu.svg`

Summary:

- Profiling binary size: `78,971,224` bytes.
- Production binary restored afterward: `6,680,460` bytes.
- p99 work during profiled run: `3206us`.
- Work frames over `16.7ms`: `0`.

Interpretation:

The flamegraph artifact is available for deeper symbol-level work. The
profiled run itself did not introduce frame-budget failures.

## Catalog And Storage

### Warm Catalog Startup

Command:

```bash
scripts/profile-warm-catalog-start.sh PERF20260629-WARM --replace-label --iterations 5
```

Rows:

- `history/toolchain-bench/results-warm-catalog.tsv`

Summary:

- First frame: `22, 24, 25, 28, 24ms`.
- Full readiness: about `2353-2357ms`.
- Main warm-catalog construction/relevant row cluster: about `280-292ms` plus
  `141-154ms` subphase in the recorded row format.

Interpretation:

Warm boot is user-friendly. The remaining warm-path concern from static review
is side-effectful navigation projection repair: this run did not isolate a
missing-projection case, so that remains a targeted test to add.

### First Scan

Command:

```bash
scripts/profile-first-scan.sh PERF20260629-FIRSTSCAN --skip-build --replace-label
```

Rows:

- `history/toolchain-bench/results-first-scan.tsv`

Summary:

- First frame: `73ms`.
- First candidate: `2.193s`.
- First discovery: `2.292s`.
- Scan/classify stage: `33.774s`.
- RAM catalog ready: `40.282s`.
- RAM catalog construction: `5.250s`.
- Projection creation: `269ms`.
- Import total: `9.978s`.
- SQLite publish: `517ms`, copy `511ms`, `6,856,704` bytes.
- DB saved: `51.655s`.
- DB count: `9237`.

Interpretation:

First scan passes the current `41s` RAM-catalog gate, but with less than one
second of headroom. The best cold-start target is RAM catalog construction and
scan/classify, not final exFAT publish.

### Library Save

Command:

```bash
scripts/profile-library-save.sh PERF20260629-LIBSAVE --iterations 5 --replace-label
```

Rows:

- `history/toolchain-bench/results-library-save.tsv`

Summary:

- DB size: `6,856,704` bytes.
- Publish totals: `559ms`, `576ms`, `547ms`, `675ms`, `552ms`.
- Copy times: `554ms`, `571ms`, `542ms`, `670ms`, `548ms`.

Interpretation:

Sequential exFAT publish is healthy. Reducing small write/sync surprises is
more important than optimizing this copy path.

### Library I/O

Command:

```bash
scripts/profile-library-io.sh PERF20260629-LIBIO --replace-label
```

Rows:

- `history/toolchain-bench/results-library-io.tsv`

Summary:

- Scan: `2.563s`.
- Discover: `2.094s`.
- Classify: `2.437s`.
- Import: `8.242s`.
- SQLite publish: `538ms`, copy `534ms`.
- Import subphases:
  - precompute catalog: `2.279s`.
  - metadata load: `1.156s`.
  - insert games total: `1.696s`.
  - materialize arcade UI: `1.217s`.

Interpretation:

The big cold-path costs are CPU/catalog work: precompute, metadata load,
materialization, and insert loops. The SD-card publish is a small part of the
total.

## Media

### Screenshot Save

Command:

```bash
scripts/profile-screenshot-save.sh PERF20260629-SAVE --system neogeo --iterations 10 --replace-label
```

Rows:

- `history/toolchain-bench/results-screenshot-save.tsv`

Summary:

- Pack size: `4,973,975` bytes.
- Total save times: `567, 388, 519, 404, 514, 409, 392, 407, 386, 402ms`.
- Parent sync: consistently `7ms`.

Interpretation:

Large sequential media writes are fine. Runtime media update risk is more about
global/external sync and interaction timing than raw write speed.

### Media Cold Boot

Command:

```bash
scripts/profile-media-cold-boot.sh PERF20260629-MEDIA-COLD --skip-build --replace-label
```

Rows:

- `history/toolchain-bench/results-media-cold-boot.tsv`

Artifacts:

- `build/media-cold-boot/PERF20260629-MEDIA-COLD.log`
- `build/media-cold-boot/PERF20260629-MEDIA-COLD-snapshot/fb0.png`

Summary:

- Validity: `valid=1`.
- Arcade media: first `2.880s`, last `49.081s`.
- Saturn media: first `36.938s`, last `49.115s`.
- Neo Geo media: first `49.035s`, last `52.964s`.
- All three systems reached `terminal=done`.
- UI rows and visible progress were seen for all three systems.

Interpretation:

The user-visible media-update flow works. Because this path overlaps catalog
reset/rebuild and network/download/save, it is a prime candidate for a future
thread/I/O sampler. It also exposed one benchmark sequencing caveat: a
launch-prep run immediately after media-cold briefly saw the DB as unavailable;
the DB was queryable on retry.

## Launch

### Launch Handoff

Command:

```bash
scripts/profile-launch-handoff.sh PERF20260629-LAUNCH --replace-label --iterations 5
```

Rows:

- `history/toolchain-bench/results-launch-handoff.tsv`

Summary:

- `launch_action_to_loading_us`: `47.889-55.739ms`.
- `max_frame_gap_us`: `16.777-21.916ms`.
- Loading frames before result: `48`.
- Failure recovery: about `2.94-3.02ms`.
- Handoff wait: about `750ms`, as expected for slow-fail mode.

Interpretation:

Failure-mode handoff recovery is stable. A successful launch benchmark remains
an instrumentation gap.

### Launch Prep

Commands:

```bash
scripts/profile-launch-prep.sh PERF20260629-LAUNCH-WARM-RERUN --replace-label --scenario warm --iterations 5
scripts/profile-launch-prep.sh PERF20260629-LAUNCH-COLD --replace-label --scenario cold --iterations 3
```

Rows:

- `history/toolchain-bench/results-launch-prep.tsv`

Summary:

- Warm rerun: `60` samples, `0` errors, p50 `49us`, p95 `2997us`.
- Cold: `36` samples, `0` errors, p50 `22us`, p95 `3008us`.
- Structured Neo Geo launch plans are near-zero I/O.
- AmigaVision descriptor writes dominate p95 with small `4096` byte writes.

Interpretation:

Launch prep is not a hot bottleneck. If polishing further, focus on avoiding or
batching the small AmigaVision descriptor writes.

## Prioritized Optimization Backlog

1. Keep catalog load read-only.
   Remove or defer navigation projection repair from
   `load_arcade_catalog_from_sqlite_at`. Write projection repairs in an
   explicit background/maintenance path after first usable UI.

2. Add dual-core visibility to benchmark scripts.
   Sample `/proc/<pid>/task/*/{stat,status,sched}` during preview, arcade,
   media-cold, and first-scan runs. Current frame traces show timing but not
   thread/core pressure.

3. Reduce cold first-scan RAM-catalog cost.
   `library_ready=40.282s` is under the 41s gate but close. Focus on the
   `5.250s` RAM catalog construction and the scan/classify path.

4. Replace runtime external `sync` calls.
   Media publish should use file and parent directory `sync_all`, matching the
   SQLite pattern, instead of shelling out and risking global sync behavior.

5. Benchmark arcade list copy shape.
   Current p99 present is fine, but the list still copies `704` rows. Test dense
   one-rect list copy versus segmented selection-frame copy.

6. Throttle preview prefetch by frame pressure.
   Selected preview is fine; prefetch queue ages can exceed a second under
   motion. Keep selected responsive and pause prefetch on missed frame/work
   pressure.

7. Add a successful launch-handoff benchmark.
   The existing handoff benchmark is useful but failure-biased. Add a safe
   success-mode row for first Main ack and successful handoff timing.

