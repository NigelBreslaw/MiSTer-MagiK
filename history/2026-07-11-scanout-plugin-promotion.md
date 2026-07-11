# Scanout plugin promotion evidence — 2026-07-11

## Confirmed cause

The proven cacheable allocations were still packaged as
`mister_magik_plugin_probe.ko`. Its diagnostics interface was initialized
before scanout and the module still attempted the obsolete bare-misc-device
`dma_alloc_wc` experiment. A production boot could therefore load successfully
without a usable `/dev/mister-magik-scanout` device.

## Before / after

- Before: 0 production-named kernel modules; scanout allocation failure was
  warning-only; the probe attempted one unsupported WC allocation.
- After: 1 `mister_magik_scanout.ko`; scanout/platform allocation is mandatory;
  0 WC scanout allocations or cacheability aliases; the old `.ko` filename and
  `/dev/mister-magik-plugin-probe` ABI remain byte-identical compatibility
  surfaces for one release.
- Performance p99 remains the production baseline because ownership/post is not
  selected yet: Home 6,888 us; Arcade 3,736 us; preview 2,469 us.

## Tests

```text
scripts/build-plugin-probe-module.sh
name:        mister_magik_scanout
description: MiSTer MagiK stock-kernel scanout plugin
vermagic:    5.15.1-MiSTer SMP mod_unload ARMv7 p2v8
sha256:      7cafc7f73943402b0325a3ff51b5e7f1c064eaf042ca9f2dcedf340f8ecf86db
```

The production and compatibility filenames have the same SHA-256. The build
uses the stock MiSTer 5.15 source and creates a loadable module only; no kernel
image, configuration, or fork is shipped.

## Evidence artifacts

- `build/plugin-probe/mister_magik_scanout.ko`
- `build/plugin-probe/mister_magik_plugin_probe.ko`
- `build/plugin-probe/modinfo.txt`
- `build/plugin-probe/SHA256SUMS`
- `kernel/plugin-probe/Makefile`
- `kernel/plugin-probe/mister_magik_plugin_probe.c`
- `history/2026-07-11-production-zero-copy-baseline.md`
