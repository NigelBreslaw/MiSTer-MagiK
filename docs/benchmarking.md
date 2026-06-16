# Benchmarking And Profiling

This document defines current benchmark policy. Dated measurement logs live in
`history/`; use them as evidence, not as the command surface.

## General Rules

- Use RGB565 for production launcher and arcade conclusions.
- The UI/app benchmark path is RGB565-only. `MISTER_FB_FORMAT=8888` is ignored
  by `mister-magik-fb ui ...`; use explicit low-level diagnostics such as
  `fb-format-smoke 8888` for framebuffer/color-route experiments.
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
- `turbo-hold` - fast continuous movement.
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
scripts/profile-preview-transition-mega.sh LABEL --deploy-device
```

These scripts write `/media/fat/mister-magik/launcher.env`, send
`mister_magik_restart_launcher`, and lock the real launcher on Arcade with:

```text
MISTER_LAUNCHER_START_SCREEN=arcade
MISTER_LAUNCHER_LOCK_SCREEN=arcade
MISTER_LAUNCHER_BENCH_SCENARIO=held-scroll|turbo-hold|preview-step-hold|idle
MISTER_PREVIEW_SCROLL_TRACE_SECS=N
```

Preview transition policy:

- Default real-app preview transition is `fade`.
- Add new transition experiments as new `MISTER_PREVIEW_TRANSITION` names rather
  than replacing existing effects.
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

Generated MagiK caches live under:

```text
/media/fat/_Arcade/media/screenshot-magik
```

Only generated cache directories should be deleted/recreated. Runtime preview
loading is raw565-oriented; build and deploy caches from the Mac with:

```bash
tools/mister preview-cache-build
```

`preview-cache-build` writes resized PNGs, `.rgb565` files, and the sibling
compressed LZ4 block archive. The real app auto-detects a sibling raw565 raw
pack first, then falls back to the compressed archive or individual `.rgb565`
files.

The library scanner must not walk screenshot/cache media directories, read
`gamelist.xml`, or probe normal PNG/JPG screenshots for metadata.

Relevant docs:

- `history/2026-6-13/arcade-screenshot-cache-workflow.md`
- `history/2026-6-14/library-scanner-preview-archive-pruning.md`

## Effect Benchmarks

List effects on device:

```bash
mister-magik-fb camera-effects
mister-magik-fb sprite-effects
mister-magik-fb text-effects
mister-magik-fb raster-effects
mister-magik-fb transition-effects
mister-magik-fb preview-transitions
```

Run effect benchmarks:

```bash
scripts/profile-camera-effects.sh LABEL --mode mega --segment-secs N
scripts/profile-sprite-effects.sh LABEL --mode mega --segment-secs N
scripts/profile-text-effects.sh LABEL --mode mega --segment-secs N
scripts/profile-raster-effects.sh LABEL --mode mega --segment-secs N
scripts/profile-transition-effects.sh LABEL --mode mega --segment-secs N
```

Catalog docs:

- `history/2026-6-13/camera-effects-catalog.md`
- `history/2026-6-13/sprite-effects-catalog.md`
- `history/2026-6-13/text-effects-catalog.md`
- `history/2026-6-13/raster-effects-catalog.md`
- `history/2026-6-13/transition-effects-catalog.md`

## Toolchain And Scene Benchmarks

General scene and toolchain benchmark entrypoints:

```bash
scripts/bench-toolchain.sh LABEL --replace-label
mister-magik-fb scenes
mister-magik-fb ui <scene> <secs>
```

`scripts/bench-toolchain.sh` appends formal results to
`history/toolchain-bench/results.tsv`. Build profiles and toolchain details live
in `magik-gui/BUILD.md`.

Bench scene documentation lives in `magik-gui/ui/bench/README.md`.

## Library Benchmarks

Use library benchmark scripts and SQL inspection rather than pulling the SQLite
database back to the host:

```bash
scripts/profile-first-scan.sh LABEL --deploy-device --replace-label
scripts/profile-library-io.sh LABEL --replace-label
scripts/bench-library.sh
scripts/mister db
scripts/mister db "SELECT count(*) FROM games"
```

`profile-first-scan.sh` deletes the production catalog database and reboots with
`scripts/mister reboot-wait`, which uses the supervised `mister_magik_reboot`
path when the Main fork is available. It records first-frame/catalog-ready
timings in `history/toolchain-bench/results-first-scan.tsv`.

`bench-library.sh` suspends the supervised launcher through `/dev/MiSTer_cmd`
while running scanner/import CLI benchmarks. Do not benchmark by directly
killing `mister-magik-fb`; that can leave the Main fork and display/OSD state
out of sync.

`profile-library-io.sh` runs one scanner/import benchmark while sampling
process CPU ticks, process I/O bytes, system CPU/iowait, and SD-card diskstats
once per second. Use it before claiming that a scanner/import change is CPU- or
I/O-bound.

Set `MISTER_LIBRARY_BENCH_CHANGED_REFRESH=1` only on disposable roots when
measuring changed-refresh behavior; it creates a synthetic candidate file.

Use `scripts/bench-library.sh LABEL --precount` only to measure the cost of a
pre-scan candidate count for determinate discovery progress. Use
`--sqlite-build-dir /tmp` only to benchmark the opt-in tmpfs SQLite build path.
