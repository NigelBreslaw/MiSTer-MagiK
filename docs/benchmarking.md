# Benchmarking And Profiling

This document defines current benchmark policy. Dated measurement logs live in
`history/`; use them as evidence, not as the command surface.

## General Rules

- Use RGB565 for production launcher and arcade conclusions.
- Use `MISTER_FB_FORMAT=8888` only for framebuffer/color-route diagnostics.
- Start visual benchmarks from a clean display-owner state. If stock OSD/menu is
  visible over the benchmark, the run is invalid even if the framebuffer PNG
  looks correct.
- Beware contaminated 30fps/vsync cadence after repeated restarts or immediate
  post-deploy runs. Settle, reboot, or rerun before declaring regressions.
- Do not compare short runs whose first seconds show `fps ~ 30` unless that is
  the behavior under test.

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

The real app auto-detects a sibling raw565 raw pack first, then falls back to
the compressed LZ4 block archive or individual `.rgb565` files.

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
scripts/bench-library.sh
scripts/mister db
scripts/mister db "SELECT count(*) FROM games"
```

Set `MISTER_LIBRARY_BENCH_CHANGED_REFRESH=1` only on disposable roots when
measuring changed-refresh behavior; it creates a synthetic candidate file.
