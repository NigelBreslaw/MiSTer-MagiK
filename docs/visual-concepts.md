# Mini-MagiK visual concepts

Device-only HDMI concepts use the existing Mini delivery and native test bridge.
The full launcher is not linked. No concept is device-qualified until its two
unprofiled windows, authoritative captures and physical animation review pass.

```
scripts/magik concept diagnostic
scripts/magik check concept --app mini-magik --concept diagnostic
```

The interactive session accepts `select NAME`, `preset default|reduced`, `pause`,
`resume`, `step`, `restart`, `capture`, and `quit`. Code changes require an
incremental rebuild; selection and presets do not. Ctrl-C and EOF restore Main.
Test sessions retain the native service's bounded lifetime.

Checks use two 30-second windows with two seconds of device-clock warmup each.
Separate `--profile` uses one ten-second attribution window. Streams and captures
must be off during qualification. Results retain binary identity, configuration,
physical cadence, CPU and RSS. Required gates: zero physical repeats and latch
failures, nominal 60 Hz, CPU below 150%, RSS at most 128 MiB.

## Implementation checklist

- [x] Physical cadence and CPU measurements
- [x] Portable contract and cached Mini session
- [x] Host controls and explicit concept checks
- [x] Shared fixtures
- [x] Point-cloud morph
- [x] Depth and parallax
- [x] Satin light sweep
- [x] Mirror floor
- [x] Pixel dissolve
- [x] Starfield and comets
- [x] Palette aurora
- [x] Texture tunnel
- [x] Raster waves
- [ ] Wireframe terrain
- [ ] Physical qualification and visual review

`point-cloud-morph`: independently selectable; default and reduced presets.

`depth-parallax`: independently selectable; default and reduced presets.

`light-sweep`: independently selectable; default and reduced presets.

`mirror-floor`: independently selectable; default and reduced presets.

`pixel-dissolve`: independently selectable; default and reduced presets.

`starfield-comets`: independently selectable; default and reduced presets.

`palette-aurora`: independently selectable; default and reduced presets.

`texture-tunnel`: independently selectable; default and reduced presets.

`raster-waves`: independently selectable; default and reduced presets.
