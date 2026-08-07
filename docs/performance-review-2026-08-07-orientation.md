# Orientation transition performance baseline — 2026-08-07

## Scope and identity

This baseline covers the real Settings view at `hdmi-1280x720p60` and the fixed
Normal → Clockwise → Counterclockwise → Normal → Counterclockwise → Clockwise
→ Normal route. It records performance data only. The device retained 720p
after every run.

- Installed MagiK revision: `88cc6e292cdac1ffcad282e602925eaf37a55300`
- Installed launcher SHA-256: `731c3c9e88d7948744f76a1cd39db235dd489d217de05c444952eac6ae8227e9`
- Boot ID: `d232f5af-1ecb-43be-99eb-1f562514d6a7`
- `MiSTer.ini` SHA-256: `ba9f58d555444535b4bd55f7a11645b4bbddf8433bc084f736e6116684581d63`
- Settings SHA-256: `6beeaf57ff441f2ec9660119272e33852ebe9fac6052b51f44825829588b6b2b`

The offline `runtime-analysis` build completed from clean commit `ce779ef7`.
Its full-debug ARM binary is
`apps/mister/target/armv7-unknown-linux-gnueabihf/release-device-profile/mister-magik-fb`.
It was not installed.

## Three-run unprofiled baseline

All three runs completed all six legs but failed the cadence and frame-work
limits. Every leg retained continuous accepted hidden-slot presentation, zero
latch drops, and zero latch sequence gaps.

| Leg | Median physical FPS | Median work P99/max (µs) | Median repeated-vblank drops |
| --- | ---: | ---: | ---: |
| Normal → Clockwise | 6.01 | 198,482 | 27 |
| Clockwise → Counterclockwise | 6.48 | 191,450 | 41 |
| Counterclockwise → Normal | 7.78 | 177,967 | 27 |
| Normal → Counterclockwise | 6.30 | 187,325 | 26 |
| Counterclockwise → Clockwise | 6.15 | 189,793 | 53 |
| Clockwise → Normal | 14.00 | 172,805 | 23 |

Evidence:

- `build/agent-benchmarks/orientation-transitions/1786122848/`
- `build/agent-benchmarks/orientation-transitions/1786122876/`
- `build/agent-benchmarks/orientation-transitions/1786122905/`

This baseline does not qualify 60 FPS.

## Attribution

The isolated pprof pass ran at 999 Hz for 3.264 seconds and retained 3,081
sample hits. Its folded stacks overwhelmingly terminate in `roundf` inside the
orientation frame path. The PMU pass retained 81 ordered-value records with no
dropped spans. Inverse-map cost ranged from 84.5 to 99.0 million cycles per
mapped frame across the six directed legs, with IPC between 0.47 and 0.54.
This is substantially larger than fill, crossfade, cache restoration, or
destination preparation.

`nm -C` shows that the optimized transition render loop is inlined into
`run_launcher_loop`, rather than retained as a standalone symbol. The pprof
leaf samples and phase PMU spans nevertheless identify repeated per-pixel
floating-point coordinate reconstruction and rounding as the dominant work.
That evidence selects the planned scanline-increment mapping candidate.

Profile evidence:

- `build/agent-benchmarks/orientation-transitions-profile/1786121954/pprof/`
- `build/agent-benchmarks/orientation-transitions-profile/1786121954/pmu/`

The Streamline interface is implemented, but no capture was run because this
environment has no user-supplied audited `MISTER_GATORD_PATH`.
