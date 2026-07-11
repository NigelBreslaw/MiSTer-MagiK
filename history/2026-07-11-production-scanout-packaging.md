# Production scanout packaging — 2026-07-11

## Confirmed cause

The kernel build had already promoted the module to `mister_magik_scanout.ko`,
but production deploy and boot preflight still selected only the retired
`mister_magik_plugin_probe.ko` filename. That could leave the new Main
supervisor without its preferred module artifact even though the build was
correct.

## Before / after

- Before: zero production deploys of `mister_magik_scanout.ko`; boot preflight
  required only the old filename.
- After: deploy installs the production module and a byte-identical one-release
  compatibility filename; boot preflight requires the production module.
- Performance is deliberately unchanged by packaging. Production work-p99 is
  still Home 6,888 us, Arcade 3,736 us, preview 2,469 us until device AFTER
  runs exercise the atomic path.

## Tests

- `bash -n scripts/deploy-main-mister-experiment.sh scripts/install-slint-boot.sh`
- `scripts/test-host-tools.sh`
- `git diff --check`

## Evidence artifacts

- `scripts/deploy-main-mister-experiment.sh`
- `scripts/install-slint-boot.sh`
- `docs/device.md`
- Main_MiSTer commit `7031c01`
- `history/2026-07-11-production-zero-copy-baseline.md`
