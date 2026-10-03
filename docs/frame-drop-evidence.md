# Frame-drop evidence capture

The capture is opt-in on the existing, unprofiled SNES round trip:

```sh
scripts/magik check animation-roundtrip --frame-evidence neighbors --installed-sha256 SHA
scripts/magik check animation-roundtrip --frame-evidence phases --installed-sha256 SHA
```

`off` is the default. Both modes preserve the route, FrameClock, input locks,
rendering and scheduling. Validated refresh repeats remain the drop authority;
motion is sampled before rendering can retire a trick. Do not combine this option
with `--profile`.
The same installed executable can run OFF/ON/OFF without deploying between modes.
Each command runs three fresh native test leases.

The completed measurement window contains `frame_evidence`. A three-frame
rolling buffer retains three predecessors and two successors around repeated
refreshes, missing fresh poses, motion/input/view edges and superseded posts.
Overlapping neighborhoods are merged. Periodic successful moving frames are
retained for comparison. Retention is limited to 256 records; overflow is explicit
and invalidates a full-route acceptance result. Records allocate no storage and
serialize no JSON on the frame path. The window exports only after completion.

Frame IDs are distinct from loop iteration IDs. Idle maintenance iterations can
be absent without representing a missed produced frame. Interrupted posts retain
their source/post identity as `superseded-before-confirmation`, with incomplete
confirmation evidence and zero invented refresh drops. Missing fresh poses are
separate from physical repeat counts, following the existing accounting.

Records include raw refresh/ownership counters, previous/current sequences,
telemetry read brackets, baseline resets, slot/full-seed/bytes, input generation,
menu/content/pose identity and the existing full phase timeline. The `phases`
mode also records seven UI thread CPU samples with wall-clock brackets; helper
renderer/request identity, dispatch/start/finish/receipt times, discarded request
generation; input dequeue and native capture times where available; bridge/model
allocation timings; destination reveal, lock and producer readiness state.
Dev UI actions can have no native input capture timestamp: null means unavailable,
not zero latency. Helper-ahead timestamps can precede the current frame.

Repeated calibration samples bracket Linux `CLOCK_MONOTONIC` with the app's
`Instant` epoch. All times are microseconds with explicit quantization. Logical
FrameClock time is a pose identity, not a deadline clock. Legacy "phase" fields
are ages since a host pacer hit and can be stale after idle. Neither they nor an
IRQ/confirmation receipt establish the FPGA eligibility cutoff.

UI CPU samples exclude the helper's CPU. Do not add overlapping helper/UI wall
intervals or nested raster/copy scopes. A wall-minus-thread-CPU residual includes
waiting, descheduling and accounting uncertainty; it is not a memory-stall count.
The observer summary measures CPU sample brackets plus record selection, excluding
clock-only timestamp reads and metadata construction. OFF/ON/OFF route comparisons
are therefore required to assess the full observer effect; never subtract mean
observer overhead from one failed frame.

No FPGA/kernel timestamp ABI, scheduler tracing or PMU capture is added here.
Deadline classification remains explicitly unknown until accepted posts can be
ordered against the actual FPGA cutoff. Use the captured CPU/helper/readiness
intervals to select the next bounded diagnostic.

## Physical qualification, 2026-10-03

Implementation commits: `552246863` (neighborhoods) and `723fb2061`
(phase CPU/helper/input/readiness). Base: merged main `4b5c8a5b`.

Neighborhood executable SHA256:
`b2dab553e678b523993b0b0f28b1094c99ebd0bc7ba9aeb2d5658ef519aa19e2`.

| Mode / run | Drops, three routes | Mean moving process CPU |
| --- | --- | ---: |
| OFF, `20261003T120419Z-14dd6b3e7522` | 19 / 20 / 20 | 18.5388 ms |
| Neighbors, `20261003T120830Z-3236477cf465` | 20 / 18 / 19 | 18.4979 ms |
| OFF, `20261003T121325Z-55c20008c984` | 19 / 22 / 18 | 18.4947 ms |

All nine native routes pass. The enabled runs retain 123/121/116 frames without
overflow and account for every counted drop. Record-selection p99 is 13/12/12 us;
calibration brackets are at most 3 us wide. The first compact capture is 255,890
bytes. No observer-related CPU/cadence change is resolved by these samples;
this is not a claim of statistical equivalence or faster animation.

Phase executable SHA256:
`5566a7342082826a475a214199918bc04af0f2710b3811cc23b049b9ea836c03`.

| Mode / run | Drops, three routes | Mean moving process CPU |
| --- | --- | ---: |
| OFF, `20261003T122641Z-b9076a3b501a` | 17 / 20 / 21 | 18.4507 ms |
| Phases, `20261003T123038Z-2c7f86e47c2a` | 18 / 18 / 17 | 18.5382 ms |
| OFF, `20261003T123448Z-385fca549e97` | 17 / 19 / 17 | 18.5311 ms |

All nine routes pass. The enabled runs retain 122/114/119 frames, with complete
CPU samples for confirmed observations, valid helper event ordering and zero
retention overflow. Compact captures are 346,185/322,893/339,231 bytes. All runs
have zero latch rejections and matching post/flip totals. No superseded post
occurred in these routes; the path is covered by recorded-state tests, not a
claim that this physical campaign exercised it.

Measured phase sampler/selection p99 is 115/138/132 us. This **exceeds the proposed
100 us observer budget**. Individual CPU brackets reach 93/282/77 us maximum;
clock-calibration brackets remain at most 3 us. Retain this explicit diagnostic
mode for millisecond-scale CPU/wait discrimination. It is not qualified as a
sub-100-us deadline observer, and it must not establish a cause for a borderline
frame. The unchanged-neighbor mode is the lighter reference; default stays OFF.
The matched process CPU/drop distributions resolve no reproducible large penalty,
but three routes per mode cannot establish absence of a small effect.

## What the new data establishes

The following observations are from phase repeat 0, with the same pattern in
other repeats where stated. CPU brackets and quantization are retained in the
raw records. Wall-minus-thread-CPU is left as an unclassified residual.

| Observed sequence / activity | Evidence | Interpretation |
| --- | --- | --- |
| 160 / card | Render wall 18.624 ms; UI CPU 14.494 ms; residual approximately 4.12–4.15 ms; helper run-queue delay 4.089 ms | Helper scheduling contributes to this critical interval. The competing task/IRQ is still unknown. |
| 190 / card | Render wall 18.109 ms; UI CPU 11.046 ms; residual approximately 7.05–7.08 ms; helper run-queue delay 4.131 ms | The helper scheduling pattern recurs, but does not explain every off-CPU/wait component. |
| 374 / SNES hub entry | UI CPU before post 33.839 ms; prepare 17.541, render 10.254, custom draw 4.323, plan/copy/post 1.721 ms | Foreground execution spans more than two refresh budgets. Prepare includes 12.840 ms bridge wall; other repeats have 12.520/12.629 ms bridge wall. Model replacement/allocation counters are zero here. The model-projection timer was disabled in this build, so its zero is unavailable evidence, not zero execution time. |
| 442 / Games entry | Render UI CPU 7.409 ms; custom draw 16.212 ms; total before post 26.275 ms | Substantial foreground work exists outside card rendering. Other repeats' custom draw is 15.997/20.160 ms. |
| 483 / Games return | Render UI CPU 15.277 ms; custom draw 18.290 ms; total before post 36.077 ms | A foreground execution overrun; other repeats have roughly 15.2 ms render plus 18.6–18.9 ms custom draw. The exact sub-operation inside custom draw needs a narrower span before optimization. |
| 225 / card | Post-accounting/confirmation interval has 16.079 ms UI CPU, with only about 25–37 us residual | This particular long confirmation interval is substantially on-CPU, not simply a sleeping wait. Split post-accounting from polling before assigning the operation. |
| 162 / card | Render wall 13.328 ms; UI CPU 13.165 ms; prepare-to-post wall 14.421 ms, yet one physical repeat | Still phase-ambiguous. Adjacent data and host read brackets do not supply the missing FPGA acceptance/cutoff timestamp. |

These records justify profiling the known foreground bridge/raster/custom-draw
operations and targeting scheduler tracing at the observed helper gaps. They do
not prove a specific background competitor, DRAM bottleneck, late latch, or a
single optimization that guarantees zero drops. Exact cutoff attribution still
requires the conditional FPGA/platform evidence described above. Fresh zero-drop
60 fps across the whole route remains unachieved.

Raw runs and derived per-drop analysis are ignored under `build/magik-results/`;
logs and the analysis script are ignored under `outputs/frame-drop-evidence/`.
The new capture made no artwork, duration, pose, input, scheduling or drop-gate
changes. Focused checks pass: four bounded-capture recorded-state tests, four
parallel helper/pixel tests, four FrameClock tests, frontend/support Clippy,
frontend tooling and no-UI compilation, and eleven host tests. QEMU/host results
are correctness evidence; the tables above are physical MiSTer measurements.


The user's bridge question exposed an attribution gap: the existing projection
clock was enabled only by the separate system-entry profiler. Phase capture now
enables it explicitly, reports unavailable timing as null, and measures six
full-bridge scopes: models/presenters, pad/clock, layout, loading/confirmation,
preview and setup. A full bridge is one transition-entry batch, not a cumulative
route total or work done for each native card column. These scope times are wall
time; foreground phase CPU provides the execution/wait context.

Bridge follow-up executable SHA256:
`29bc98f89ab0c0b140b8ce63409982df61f905a3260b78ca054d0bda373c4996`,
commit `a182ef3d9`. Native three-route capture:
`20261003T125657Z-d7ac128e71dc`, passed; drops 18/17/22 and mean moving
process CPU 18.5947 ms. This is an attribution run, not a speedup result or a
replacement for the earlier same-binary OFF/ON/OFF qualification.

The single slow entry bridge call in each route breaks down as follows:

| Repeat | Whole batch | Models/presenters | Pad/clock | Layout | Loading/confirmation | Preview | Setup |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 0 | 11.696 ms | 11.312 ms | 47 us | 4 us | 15 us | 291 us | 16 us |
| 1 | 12.474 ms | 12.180 ms | 49 us | 3 us | 11 us | 202 us | 16 us |
| 2 | 12.926 ms | 12.638 ms | 46 us | 3 us | 10 us | 199 us | 17 us |

These are individual first-entry batches, not cumulative totals. Scope sums
include timer resolution/overhead and can differ slightly from the outer timer.
The generic model/presenter updater accounts for approximately 97% of the cost.
It publishes the destination page needed by the transition; native card column
rendering does not need these updates. It also touches settings, menu and Arcade
presentation state. The hub recent/favourite functions each iterate the active
collection (`launcher.rs`), but their share of the measured model/presenter
scope is not established. Do not attribute all 12 ms to those scans or to simple
property assignment without a narrower measurement.

Final control/scanout check on the phase executable:
`20261003T123925Z-2df7dd7ba8aa`, passed A, Back, Select action and locked-trick tap.
Hub, games, Select return and root static pixels match the earlier qualified
PR #217 captures exactly; differences are limited to the clock (and changing
preview region when applicable). Captures are FPGA-latched RGB565, not fb0.
The later bridge-only timing change passes 20 bridge behavior tests, recorded
capture tests, UI/tooling Clippy and no-UI compilation; it does not alter the
bridge's state updates or rendering operations. Physical joystick qualification
and CRT/portrait qualification remain outside this HDMI capture campaign.


## Publication audits

Deletion commit `8860dd8a4` removes the duplicated last-frame copy and two
unnecessary serializer visibility changes. The rolling buffer is the authority
for the preceding observation. Simplification commit `c3694256c` centralizes
observer timing, rejects incompatible commands before device dispatch and leaves
unmeasured light-bridge scopes null rather than inventing zero durations.

The audit found and fixed independent missing-pose and view/input/readiness
retention triggers. The earlier missing-pose test was masked by an outcome change;
isolated state-sequence regressions now cover each trigger without a second
edge. These fixes change diagnostic retention, not physical drop accounting.

Focused validation: five collector tests, fourteen host tests, frontend UI/tooling
and support Clippy, no-UI check and ARM device build pass. Final native capture:
`20261003T132324Z-a673263edff1`, executable SHA256
`672619fa5ebb7920201dcaeb9ac87f7ddfeb90e3a9244b707382cacf065c1275`.

| Repeat | Drops | CPU / moving presentation | Retained frames | Sampler/selection p99 |
| --- | ---: | ---: | ---: | ---: |
| 0 | 20 | 18.5438 ms | 126 | 146 us |
| 1 | 19 | 18.5506 ms | 132 | 133 us |
| 2 | 21 | 18.5965 ms | 133 | 111 us |

All three routes pass with zero retention overflow, complete confirmed-frame CPU
evidence, valid helper ordering, zero latch rejections and matching posts/flips.
Mean process CPU is 18.5636 ms per moving presentation. This does not establish
a speedup or resolve the existing observer-budget/FPGA-cutoff limitations.


## Review corrections

Review of the launcher wiring found gaps that the original collector-only tests
did not cover. These corrections preserve pixels, FrameClock and input behavior.

| Finding | Correction and regression evidence |
| --- | --- |
| Motion missing on telemetry failure; inconsistent superseded motion | One pre-raster navigation/card snapshot supplies motion independently of telemetry. An actual Arcade direction-input sequence exercises failed observation, superseded post, Home restart and replacement. |
| Missing window-opening frame | Loop-entry candidates cover warmup and the iteration that opens inside `Session::tick`; only an open window retains them. A real Session tick/open/drop/close sequence checks the first record against the window total. |
| Discarded input-priority raster | The restart path retains its produced ID, raster timeline and input identity before continuing, with zero invented refresh drops. The launcher sequence checks consecutive produced IDs across abandonment and replacement. |
| Stale helper dispatch origin | New jobs timestamp dispatch after stale work is drained. Completions carry that actual dispatch; the existing source-replacement/pixel-parity sequence also compares it with the discarded worker's final clock sample. |
| Final trick labelled settled | Pose, content and motion are sampled before raster retirement. The actual asynchronous level-change test renders the terminal trick frame and verifies it retains `level-deal`, full progress and the lock. |
| Stale first read bracket | Baseline telemetry, attempt ID and nullable bracket share one observation value. An unbracketed replacement explicitly has a null bracket, never zeros or an older read. |
| Invalid mode partially applies request | Duration and evidence mode validate before deleting the request or mutating session fields. A bad mode preserves the request and active state; correcting it starts the new measurement. |
| Timing cleanup | The four mandatory helper Instants are plain values, renderer IDs use AtomicU64 and light/full bridge model timing uses the same optional clock helper. |

Candidates take the first CPU sample during warmup to retain a real loop-entry
sample if tick opens the window in that iteration. Warmup records and their
observer costs are not retained. This explicit phase-mode overhead remains
subject to the existing observer-budget limitation.

The cited 4.089/4.131 ms helper delays are **schedstat run-delay deltas during
helper execution**, not dispatch-to-start times. The saved repeat-0 records at
sequences 160/190 have no discarded helper (`discarded_helper_us = 0`,
`discarded_generation = null`). Their dispatch-to-start times are 72/403 us.
The stale-dispatch bug therefore does not invalidate these two run-delay
observations. Dispatch-to-start values from old captures that did discard work
are unqualified and must not be interpreted as scheduling delay.

The two PR #217 review points are already fixed in the merged base
`4b5c8a5bc`: oversized watch metrics use the JSON body without interrupting logs,
frames or following updates, and NEON parity explicitly exercises opaque and
rounded-cap source columns, with a minimum fast-path coverage assertion. The
large-watch streaming regression was rerun and passes.

Review-fix native validation: `20261003T140800Z-dd9dbce33b7f`, all three routes
pass using the same executable SHA256
`3a3e1842738a203317257a2e22e8ca6bcfd6c6118beb889fb7d95c70a44b4645`
(parent `a4d018501` plus the uncommitted review fixes).

| Repeat | Drops | CPU / moving presentation | Retained frames | Sampler/selection p99 |
| --- | ---: | ---: | ---: | ---: |
| 0 | 16 | 18.6389 ms | 124 | 137 us |
| 1 | 19 | 18.6212 ms | 123 | 101 us |
| 2 | 18 | 18.5329 ms | 121 | 132 us |

Mean moving process CPU is 18.5977 ms. Every route has zero retention overflow
and latch rejections, matching posts/flips, complete confirmed-frame CPU/helper
evidence and retained frame drops equal to its window total. Terminal trick
records remain `level-deal` at full progress with motion and the lock retained.
These are reporting-correction checks, not a speedup claim. Hardware did not
exercise telemetry read failure or input-priority raster abandonment in this
route; the shared launcher sequence regressions cover those paths.

Focused validation passes: 21 support tests, four helper/profile/pixel tests,
launcher navigation/restart and bracket regressions, the actual asynchronous
trick sequence, nine host CLI tests, UI/tooling/support Clippy, the large-watch
streaming regression and the ARM release build used above.
