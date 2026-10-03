# Native app animation misses — 2026-10-03

The current bridge-refactor executable was measured on MiSTer Dev, with three
complete journeys for each of seven navigation routes, plus three ten-second
screensaver windows. App rendering and timing
code were unchanged during this campaign. Measurements use native FPGA scanout counters. The main findings are expensive document-page
transitions and a motion-boundary reporting problem in Computers.

## Three-pass results

| Route | Raw reported misses | Repeated motion refreshes | Stale card poses | Mean process CPU per moving presentation |
| --- | --- | --- | --- | ---: |
| Root carousel: six steps right, six left | 37 / 36 / 37 | 1 / 0 / 1 | 36 / 36 / 36 | 22.724 ms |
| Consoles → Nintendo → SNES → Games → return | 7 / 5 / 7 | 7 / 5 / 7 | 0 / 0 / 0 | 18.247 ms |
| Computers → Sinclair → ZX Spectrum → Games → return | 141 / 155 / 140 | 141 / 155 / 140 | 0 / 0 / 0 | 18.639 ms |
| Handhelds → Nintendo → Game Boy → Games → return | 6 / 6 / 6 | 6 / 6 / 6 | 0 / 0 / 0 | 15.554 ms |
| Arcade hub, sections, list, drawers and return | 6 / 6 / 6 | 6 / 6 / 6 | 0 / 0 / 0 | 9.179 ms |
| Global Favourites and return | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | No tracked motion |
| Settings, About, Licenses, license text and return | 238 / 241 / 239 | 238 / 241 / 239 | 0 / 0 / 0 | 20.045 ms |
| Screensaver, ten seconds | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 10.915 ms |

Raw `dropped_frames` adds two different counters: repeated owned refreshes during
motion and presentations reusing the same card-source generation while the card
session reports animation. These are not interchangeable or necessarily unique
visible missed frames. Root has 36 reused-source presentations per run, three per
tap, and only 1 / 0 / 1 repeated refreshes. Their visual significance needs a
focused pose trace; this bench does not prove why those source generations repeat.

The CPU column includes background app threads and both cores, sampled during
published UI motion. It is CPU cost, not single-thread wall time or a frame deadline.
Different routes have different numbers and types of animations; this is a workload
comparison, not an optimisation before/after claim.

## Settings is the largest captured refresh-miss hotspot

| Transition | Misses: runs 1 / 2 / 3 |
| --- | --- |
| Root → Settings | 1 / 1 / 1 |
| Settings → display choices | 0 / 0 / 0 |
| Display choices → Settings (cancel) | 0 / 0 / 0 |
| Settings focus → About | 0 / 0 / 0 |
| Settings → About | 38 / 38 / 38 |
| About → Licenses | 39 / 39 / 39 |
| Licenses → license text | 40 / 40 / 40 |
| License text → Licenses | 39 / 40 / 39 |
| Licenses → About | 38 / 39 / 38 |
| About → Settings | 40 / 41 / 41 |
| Settings → Root | 3 / 3 / 3 |

A retained first-pass frame records 20.037 ms of pre-custom UI rendering and
2.491 ms of custom drawing, with two owned refreshes between observations and
one repeated refresh. Other retained transition records show the same pattern.
This exceeds the 16.7 ms refresh budget; the activity label is `system-transition`,
with no card work attached to that record. This locates a budget overrun without
proving which UI-render substage is responsible.

The ordinary dropped-record buffer retained evidence for 64 misses per Settings
run; 173 / 176 / 174 records were omitted. The complete per-step counter deltas
sum to 238 / 241 / 239, but the retained timelines do not cover every miss. A
follow-up should measure the six page transitions separately to retain complete
evidence before choosing a rendering change.

## Computers totals include unqualified idle gaps

| Run | Raw refresh misses | Short-interval records | Long-gap records requiring qualification |
| --- | ---: | ---: | ---: |
| 1 | 141 | 16 | 125 |
| 2 | 155 | 16 | 139 |
| 3 | 140 | 15 | 125 |

Long-gap classification means more than 100 ms between the previous telemetry
observation and the recorded frame beginning; it is a diagnostic grouping, not a
corrected drop count. In run 1, the single 125-miss record spans a 2.065-second
gap before frame begin. Its UI render takes only 15.732 ms. The previous
step returned a fresh `ui_motion=false` sample at 19.670 seconds, between the
previous observation at 18.952 seconds and frame begin at 21.018 seconds. The
counter therefore includes settled time in an interval subsequently labelled motion.

Earlier completed attempts with a two-second host pause reported 204 / 203
misses, including long-gap records near 190. With the shorter polling delay, the
same executable reports 141 / 155 / 140. This strongly supports idle contamination
rather than attributing the whole large count to card rendering. Do not treat the
short-interval remainder as a corrected whole-route total: the long interval may
also contain a genuine deadline miss. Motion/observation baselines need repairing
before using the Computers totals to compare optimisations.

## Other per-step findings

- SNES: hub entry 1 / 1 / 1, Games opening 1 / 1 / 1, Games → Nintendo
  3 / 3 / 4. Remaining misses occur during card browsing/entry.
- Game Boy: hub entry, Games opening and Select return each miss one refresh;
  hub → Nintendo misses three. Other steps report zero in all three passes.
- Arcade: hub entry, Games opening and Select return each miss one refresh;
  hub → root misses three. Hub section changes, list scrolling and both drawers
  report zero in all three passes.
- Favourites: enter/return assertions pass, but `motion_starts=0` and
  `moving_presentations=0`. The zero counters do not qualify animation cadence.

## Screensaver

All three ten-second windows presented 600 frames with 600 matched latch posts/
flips, zero repeated-refresh or stale-pose misses, and zero latch rejections.
Process CPU per moving presentation was 10.928 / 11.045 / 10.774 ms.

## Reproduction and validation

```sh
scripts/magik check animation-app --installed-sha256 a18064e8ce99222d87fed68fba9e7965a6c920de51a271e90467b9ea10b3e691
```

`MAGIK_ANIMATION_ROUTES=arcade,favourites,settings` selects a subset. Each route
uses its own native test lease and checks the artifact hash, process identity,
vsync-locked clock, destination state, fresh idle samples and counter sums.
Measurement windows are 45 seconds for root/SNES, 35 for Computers/Handhelds/
Arcade/Settings and 15 for Favourites. Detailed phase/kernel profiling is off.

All 21 complete journeys passed individually, with matched latch posts/flips
and zero latch rejections. The first 15 are retained in three runs whose overall
pytest result also includes a later failed fixture attempt; only completed
`animation-app` results are included. Failed selectors, control assertions and
lease/window attempts are excluded. The final six-journey run passed as a whole.
Fourteen focused host tests pass.

Measured app source: `b2cd60791`, based on `6fc86fe7f` (PR #219).
Executable SHA-256:
`a18064e8ce99222d87fed68fba9e7965a6c920de51a271e90467b9ea10b3e691`.

Retained complete journey runs:

- `20261003T181924Z-4a6d8ea14aa8`: root and SNES.
- `20261003T183637Z-071474473054`: Computers and Game Boy.
- `20261003T184718Z-36189d80581d`: Arcade.
- `20261003T185146Z-b5e511d6679c`: Favourites and Settings.
- Screensaver: `20261003T185747Z-fa3f3cd914b2`,
  `20261003T185823Z-f2f474f3d5c4`, `20261003T185859Z-938c0dbbd5ed`.

Raw measurements remain ignored under `build/magik-results/`; the aggregate is
`outputs/bridge-refactor/app-animation-summary.json`. Measurements cover the
960×540 HDMI route and Dev action queue. Physical joystick input, CRT/portrait
output, other installed systems and populated global user lists are not qualified.

There is no trustworthy all-app missed-refresh grand total yet: the Computers
idle-gap issue and Favourites motion coverage must be resolved first.

The PR audits subsequently consolidated route configuration and navigation into
`magik/host/magik/animation_benchmark.py` and removed the separate route module.
The measured inputs, waits, windows and destination assertions are preserved.
This harness cleanup passed the focused host tests; the device evidence remains
from the recorded pre-cleanup executable and workload.
