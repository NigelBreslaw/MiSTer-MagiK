# Effect Experiments

Effect scenes are experimental visual probes. They are useful for exploring
future MiSTer MagiK transitions, camera/background effects, sprite effects, text
effects, and raster/palette ideas, but they are not part of the production
launcher UI and their results are not release benchmark evidence.

Production builds expose the real launcher, the fixed 200ms fade preview
transition, and the minimal runtime command surface. Experiment builds add:

- `ui camera-effects`
- `ui sprite-effects`
- `ui text-effects`
- `ui raster-effects`
- `ui transition-effects`
- `ui screensaver`
- `effect-bench`
- expanded preview transition variants under
  `scripts/experiments/preview/profile-preview-transition-mega.sh`

`effect-bench` supports `native`, `2x`, `half`, and `full` fill modes. All
modes now prepare RGB565 scratch pixels and present through the checked
framebuffer APIs; there is no direct raw/scaled framebuffer copy path or FPGA
scaler fill mode to preserve.

Build and deploy an experiment-enabled binary before running these tools:

```bash
scripts/deploy-rust.sh --experiments
```

The experiment scripts also accept `--deploy-device`; that path deploys an
experiment-enabled binary first. With `--skip-build`, each script preflights the
deployed binary and fails before appending TSV rows if experiments are missing.

## Scripts

```bash
scripts/experiments/preview/profile-preview-transition-mega.sh LABEL --deploy-device --segment-secs 5 --transition-ms 320
scripts/experiments/effects/profile-camera-effects.sh LABEL --mode mega --segment-secs N
scripts/experiments/effects/profile-sprite-effects.sh LABEL --mode mega --segment-secs N
scripts/experiments/effects/profile-text-effects.sh LABEL --mode mega --segment-secs N
scripts/experiments/effects/profile-raster-effects.sh LABEL --mode mega --segment-secs N
scripts/experiments/effects/profile-transition-effects.sh LABEL --mode mega --segment-secs N
scripts/experiments/effects/profile-screensaver.sh LABEL --mode mega --segment-secs N
scripts/experiments/effects/bench-effects.sh LABEL --device --replace-label
```

Use these outputs to compare experiment variants with each other. Do not cite
them as production launcher, Arcade preview, or release-readiness evidence.

## Catalog History

The dated catalogs remain useful background, but they are history rather than
current benchmark policy:

- `history/2026-6-13/camera-effects-catalog.md`
- `history/2026-6-13/sprite-effects-catalog.md`
- `history/2026-6-13/text-effects-catalog.md`
- `history/2026-6-13/raster-effects-catalog.md`
- `history/2026-6-13/transition-effects-catalog.md`
