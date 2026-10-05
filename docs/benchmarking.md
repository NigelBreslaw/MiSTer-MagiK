# Application scenarios and physical input qualification

Everyday application correctness, measurements and optional profiles use the
shared [2.0 Python framework](../magik/README.md). Real MagiK and Mini-MagiK use
the same service and result format.

```sh
scripts/magik check
scripts/magik check idle
scripts/magik check idle --profile
PYTHONPATH=magik/host uv run --project magik/host pytest magik/scenarios --magik-device -k journeys
PYTHONPATH=magik/host uv run --project magik/host pytest magik/scenarios --magik-device -k journeys --magik-profile
```

The journeys exercise Arcade selection and return, and change/restore Reduce
motion. Two unprofiled repetitions retain host-observed response times, including
RPC and accessibility polling; these are not device frame latency or an FPS
benchmark. The separate profile uses the same journeys and a device-clock
measurement window and a 15-second whole-sequence allowance. The ten-second
profile samples the journey; it need not include all actions and cleanup.
Default smoke remains unchanged and does not select them.
The existing idle and Mini motion measurements remain available.

## Launcher sampler and quantiser experiments

The real launcher has two independent opt-in switches. `MAGIK_CARD_SAMPLER_AB=axis`
adds premultiplied vertical prefilter levels to the existing horizontal mip sampler.
`MAGIK_CARD_QUANTISER=fast` selects centred Bayer noise with saturated RGB565 bit
quantisation, including four-pixel NEON edge compositing. Both switches default to
`current`; they do not change production build defaults.

```sh
MAGIK_CARD_SAMPLER_AB=axis MAGIK_CARD_QUANTISER=fast scripts/magik check motion-held --app magik
```

The held test always starts on Arcade, uses the fixed
animation clock and retains `motion-held-metrics.json` before assertions. Compare
matching render geometry, starting selection, clock, artifact hash and repeated
unprofiled windows. Analytics identifies `card_sampler` and `card_quantiser`;
profiled windows are separate diagnostics, not timing results. Reduced CPU usage
alone does not establish zero-drop cadence or visual suitability.

Vertical filtering stores up to four additional full-height prefilter families
so geometry, cropped preparation and reflection coordinates remain shared. It
therefore trades additional retained texture memory for reduced vertical aliasing.
The fast quantiser intentionally changes colour rounding and the dither texture.
Keep either switch opt-in until its appearance and physical-device behaviour are
qualified for the intended route.

The session verifies the requested sampler and quantiser against the running
binary's Analytics context before measuring. This also applies to
`MISTER_MAGIK2_PREBUILT_ARTIFACT` and installed-artifact runs: environment selectors
cannot change a precompiled binary, and mismatches fail rather than producing a
mislabelled comparison. The fast switch applies only to launcher card projection;
Arcade and cabinet compositors retain their original quantisation.

Run `scripts/magik-ci neon-parity` on a native ARMv7 or AArch64 host to compile
and execute the C parity harness both with and without `MAGIK_FAST_QUANTISATION`.
The ARM CI job runs the same command. AArch64 results establish pixel correctness,
not Cortex-A9 timing. Rust tests independently check texture-family selection,
nonzero blend weights, crops and conservative opacity bounds.

Back faces and native-size faces also use perspective projection, and native rows
shrink successive cards. They therefore retain vertical filtering; excluding those
face types would reintroduce aliasing. The current full-height representation costs
approximately five times the horizontal-only pixel storage. Compact vertical mips
remain a separate optimisation requiring new reconstruction kernels and measurements.
