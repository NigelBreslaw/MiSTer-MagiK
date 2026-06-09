# Frame Analysis Options

Status: implemented as small profiling/reporting slices through 2026-06-09.

## Runtime Instrumentation

| # | Option | Implementation |
|---|--------|----------------|
| 1 | Rename ambiguous buckets | Runtime/log/TSV wording uses `slint-render` and `fb-present`; historical bench columns remain compatible. |
| 2 | Split present work | `fb-present` is split into `cached-present` and `overlay-present`. |
| 3 | Prepare-frame bucket | `prepare` covers pre-Slint input/catalog/bridge work where available. |
| 4 | Custom drawing bucket | `custom-draw` separates project-owned draw work such as arcade list layers from `vsync-wait`. |
| 5 | Per-frame TSV v2 | `MISTER_PROFILE_FILE` writes phase, rect, pixel, byte, vsync, and dominant-phase fields. |
| 6 | Event trace | `MISTER_TRACE_FILE` writes Chrome/Perfetto JSON trace events. |
| 7 | Cheap live modes | `MISTER_PROFILE=summary`, `slow`, `full`, and `trace`. |
| 11 | Bandwidth metrics | Profile summary prints presented bytes and active copy MiB/s. |
| 18 | CPU flamegraph path | Existing `MISTER_PPROF=1`; wrapper: `scripts/cpu-flamegraph-scene.sh`. |

## Host Reports

| # | Option | Command |
|---|--------|---------|
| 8 | Stacked bar chart | `scripts/frame-profile-chart.py frames.tsv frames.svg` |
| 9 | Phase histograms | `scripts/frame-profile-histogram.py frames.tsv` |
| 10 | Dirty-region heatmap | `scripts/frame-profile-heatmap.py frames.tsv heatmap.svg` |
| 12 | Slow-frame report | `scripts/frame-profile-slow-frames.py frames.tsv --limit 12` |
| 13 | Regression comparison | `scripts/frame-profile-compare.py before.tsv after.tsv` |
| 14 | Interactive-ish HTML report | `scripts/frame-profile-report.py frames.tsv report.html --trace frames.json` |
| 19 | Report index | `scripts/frame-profile-index.py build/frame-profiles` |
| 20 | One-command profiling workflow | `scripts/profile-scene-report.sh full_motion 5 LABEL` |

## Tooling Guidance

| # | Option | Decision |
|---|--------|----------|
| 15 | Tools installable on MiSTer | Do not install packages for normal analysis. Capture low-overhead TSV/trace data on MiSTer and render reports on the host. |
| 16 | Use Linux tools selectively | Use `pprof`/`perf_event_open` only for CPU attribution after frame phases identify a CPU-bound region. |
| 17 | Flamegraph caveat | CPU flamegraphs can produce zero samples depending on MiSTer perf permissions; frame TSV/trace reports remain the primary path. |

## Typical Flow

```bash
scripts/profile-scene-report.sh full_motion 5 FM-SMOKE
open build/frame-profiles/FM-SMOKE-report.html
```

For CPU attribution:

```bash
scripts/cpu-flamegraph-scene.sh full_motion 10 FM-CPU
```
