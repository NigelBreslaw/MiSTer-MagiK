# Launcher rendering and Mini validation

Production and Mini share the high-fidelity card preparation and live Arcade
renderer. Scanout remains RGB565. The six 360x504 RGB888 sources retain precision
through filtering, lighting and final destination-space dithering. Landscape
constructs the retained faces directly, reduces each artwork once for its two
label variants and skips redundant subpixel samples in exact artwork interiors.
Native responsive layouts keep their pixel-aligned fonts and coverage.

Arcade retains the existing Home/list timeline, captured live screen data,
reversal, cancellation and navigation completion. Its two workers render row
bands split at y=311. Each owns bounded scratch; immutable mip data is shared.
The application owns thread policy. The resting cabinet uses the same RGB888
source and final quantisation. Artwork is prewarmed off the UI thread alongside
initial card preparation, and reveal workers retire after navigation completion.
Production retains its existing card render-ahead and hidden-slot publication
contract. No new affinity/priority policy, baked animation or angle atlas is used.

The old source RGB565 copies, nearest-sampled HDMI cabinet path, quality switches
and rejected complete-frame cache variants have been removed. Mini's `default`,
`rgb888` and Arcade `scanline` names select the same accepted renderer; the latter
two names remain as aliases for existing experiment commands. Other concept
workloads retain their normal `default`/`reduced` choices.

## Interactive and startup checks

```sh
scripts/magik concept launcher-cards --production-build
scripts/magik concept arcade-transition --production-build
scripts/magik check concept --app mini-magik --concept launcher-cards --production-build --bench-preparation
scripts/magik check concept --app mini-magik --concept arcade-transition --production-build --profile-preparation
```

Interactive controls: `pause`, `resume`, `step`, `restart`, `capture`, `quit`.
Startup measurements prepare a fresh scene, render one paused frame, retain
its first confirmed presentation and return to the Dev launcher. CPU profiling
starts before preparation and finishes before rendering the initial scene.
Benchmarking disables CPU sampling. No storyboard or quality-review journey runs
in startup mode. `--installed-sha256` verifies a ready running artifact instead
of building/deploying another one.

The readiness clock starts before scene construction and ends after the posted
slot is confirmed settled. It includes preparation, rendering, transfer and the
refresh wait, but excludes process launch, preceding control delivery, driver
acquisition and full-app catalog setup. CPU profile export adds overhead between
preparation and first presentation; use sampling-disabled benchmarks for readiness.

## Recorded optimisation evidence

Physical Cortex-A9 960x540 preparation profiles measured each implementation step:

| Implementation | Preparation |
| --- | ---: |
| Recorded original profile | 2078 ms |
| Construct RGB888 faces directly | 1281 ms |
| Share artwork reduction | 1112 ms |
| Exact artwork-interior shortcut | 547 ms |

The final pre-integration implementation measured 531–540 ms card preparation and
560–573 ms to first confirmed presentation across three fresh processes without
CPU sampling. Cold Arcade measured 601 ms preparation and 622 ms first confirmed
presentation. Captured initial frames matched their recorded references exactly;
face and every RGBA mip level matched independent construction. Baselines are
recorded evidence and were not rerun for later comparisons.

Before production integration, two 30-second Mini windows measured cards at
59.800/59.767 FPS with 6/7 repeats; live Arcade at 59.967/59.967 FPS with one repeat
per window. These did not pass the strict zero-repeat gate. Card misses recurred
about every 5.03 seconds at different poses. Bounded `context.late_frames` records
retain render/transfer/presentation and worker wall times, but do not establish
CPU execution versus scheduler/IRQ delay. Full scheduling diagnostics remain a
separate follow-up; no priority guesses were applied to these measurements.

Ignored evidence under `build/magik-results/`:

- `20260930T141221Z-05b1371be29b`: recorded card cadence and reference captures.
- `20260930T141542Z-092265babf15`: recorded live Arcade cadence and captures.
- `20260930T145043Z-e3548c3c2494`: original cold preparation profile.
- `20260930T150355Z-37dfc6998f54`: direct faces profile.
- `20260930T150758Z-23910b53d596`: shared reduction profile.
- `20260930T151110Z-9bf8b3e1d3d2`: interior shortcut profile.
- `20260930T151326Z-f2b02572c769`, `20260930T151352Z-ff934206adf7`,
  `20260930T151417Z-bebfe2157fc9`: final card startups and authoritative captures.
- `20260930T151514Z-b2596ab09229`: cold Arcade startup and capture.

Production integration validation is recorded with its current artifact and
live full-app measurements. Mini evidence does not qualify whole-app input,
catalog contention, CRT scanout or zero-miss production cadence.

## Production integration validation

The production build and Dev smoke passed in
`20260930T160945Z-8077ae290ade`. Focused production checks passed 29 card tests
and 33 navigation tests, including reverse/cancel and fresh snapshot binding.
The shared parallel reveal also matched serial row composition through fade,
identity and endpoint boundaries. Mini controls, layout-review example, host
scenario tests and focused Clippy passed.

Scripted full-app card motion in `20260930T161602Z-427423fe6532` completed three
five-second windows with 300 UI presentations each. Its owned-refresh counters
are not populated, so those results do not establish physically perfect 60 Hz.
Authoritative native framebuffer evidence was saved separately under
`20260930T162404Z-ec3bc0c50d17/fpga-incident/` and reviewed at 960x540 RGB565.

Collections reports its existing navigation readiness so host focus changes wait
for transition ownership to settle. Each reversible journey uses its own existing
60-second native test lease. All four production journeys passed in
`20260930T163904Z-f97d5be396a6`: two Home/Arcade/list return journeys and two
Reduce motion toggles, each restoring the original Off setting. The application
and service timeout remain unchanged. The tested artifact SHA-256 was
`ff6fc6ffd9a1c2c88bea8bcd346c741f88014f91b6111f1872e627cd28320f9a`.
