# Benchmarking policy

Input loss and latency qualification uses the typed
`scripts/agent benchmark input-integrity` workflow described in
[Unified input](input.md). Do not substitute raw SSH input injection: the gate
must traverse Main's real mapping, aggregation, proxy-v2, and kernel path.
The fixed gate compares a bounded launcher event trace against the exact injected
sequence in idle and catalog/CPU/stall scenarios. Intentional UI stalls are input
correctness stress, not rendering-cadence qualification.

`scripts/agent benchmark [SCENARIO]` is the only agent-facing performance
workflow. Scenarios are a closed typed registry rather than a flag matrix. It
never builds, deploys, or replaces platform files. The fixed `cold-boot`
scenario is the sole benchmark allowed to issue one supervised Linux reboot;
all other scenarios must leave the device boot unchanged. The
installed platform manifest and its hashes are the benchmark identity, and its
delivery reconciliation against the clean local Git HEAD must be a no-op.
Host-only benchmark tooling changes therefore do not force an identical runtime
revision, while pending runtime or platform changes remain a hard failure.

Supported scenarios:

- `screensaver` (the default)
- `cold-boot`
- `input-integrity`
- `launcher-response`
- `input-latency-lab`
- `particles`
- `particle-demo-40k`
- `particle-capacity`
- `particle-step`
- `particle-profile`
- `catalog-lifecycle`
- `launch-return`
- `launch-return-fallback`
- `modal-input`
- `navigation-transitions`
- `settings-navigation`
- `settings-navigation-pprof`
- `orientation-transition-fade`
- `orientation-transition-zoom`
- `orientation-transition-fade-pprof`
- `orientation-transition-zoom-pprof`
- `pmu-profile`
- `search`

`modal-input` restarts the coherently installed Dev launcher with a one-shot,
fixed test request and a catalog copied below
`/tmp/mister-magik/modal-input-benchmark`. It presents the real catalog upgrade
recovery dialog over the selected Arcade tile, selects `Rebuild`, and holds A
while the dialog closes. The run fails if that held A reaches the underlying
Home view, then proves that release leaves Home selected and a fresh A press
opens Arcade. Semantic snapshots and authoritative framebuffer checkpoints are
retained below `build/agent-benchmarks/modal-input/<timestamp>/`. The isolated
catalog and all one-shot environment state are removed before the ordinary Dev
launcher is restored; the installed manifest and device boot ID must remain
unchanged.

`navigation-transitions` runs the fixed scripted route Home → Arcade → Home →
Consoles → System → Consoles → Home with the Super-Scaler POC enabled. It
records per-frame process CPU, transition and overlay cost, snapshot locking,
Slint calls, status-writer overlap, preparation attribution, latch sequence
continuity, physical FPS, and drop counters for all six legs, then restores the
ordinary launcher. Evidence is written below
`build/agent-benchmarks/navigation-transitions/<timestamp>/`.

`launcher-response` is the release gate for interactive selection latency and
confirmed focus feedback. Schema v2 has no v1 compatibility parser. It drives
the exact Computers route from Acorn through Apple II, Commodore, Atari,
Sinclair, CoCo 2, DOS, Japanese, and Other with a 100 ms baseline plus rotated
50/57/64/71 ms start-to-start schedules. It also covers discrete System Hub and
Settings focus and Arcade press-to-first-motion through Main proxy v2.

Each eligible destination must have one exact pulse-on and pulse-off active
latch sequence with at least 80,000 µs between their physical confirmations.
Pulses may overlap; the final Other selection must confirm without waiting for
an older pulse deadline. Arcade's velocity list deliberately has no pulse and
is gated on first confirmed motion instead. Runs execute idle and during forced
catalog work at physical 60 Hz and 50 Hz, with zero input loss, duplication,
coalescing, reorder, overflow, desync, latch drops, protocol-v5 repeated
vblanks, or ownership loss.

Dispatch P95 must be at most 3 ms and its maximum at most 5 ms. Input-to-visible
median must be at most 12 ms; each display-rate leg independently limits P95 to
one refresh period plus 3 ms and the maximum to one refresh period plus 8 ms.
The report exposes independent input-response, pulse, integrity, and background
adoption statuses. Forced catalog adoption remains a separate below-8-ms gate,
and the overall result passes only when every status passes.

For attended diagnosis of intermittent single-press latency, set
`MISTER_LAUNCHER_RESPONSE_ISOLATED_PROFILE=1` when running `launcher-response`.
This diagnostic mode is not the release qualification: it holds the physical
display at 1920×1200p60, keeps forced background catalog refresh active, enters
Computers once, and runs Acorn→Other→Acorn four times at 600 ms start-to-start
without restarting the launcher. It retains every capture, dispatch, and exact
active-latch confirmation. The ordinary command remains the 60/50 Hz
multi-route qualification above.

`input-latency-lab` is the fixed diagnostic experiment for attributing
intermittent launcher response, not a release qualification. It switches the
physical display to 1920×1200p60 for the bounded run, then restores the prior
mode. Each arm enters the real Computers menu at Acorn and drives exactly four
Acorn→Other→Acorn cycles: 64 presses, 40 ms held, 600 ms start-to-start. The
driver and launcher share a device-monotonic epoch, so UI-thread obstruction
starts 8 ms before each scheduled press.

The first six arms are baseline with catalog refresh off, real forced catalog
refresh, monolithic 16 ms, monolithic 64 ms, cooperative 2 ms quanta totalling
64 ms, and cooperative 1 ms quanta totalling 64 ms. Three additional
forced-catalog arms compare the current CPU0/nice -10 input reader against
CPU1/nice -15 and low-priority round-robin scheduling on CPU0 and CPU1. These
reader policies require the same consumed volatile lab token and do not alter
ordinary runtime scheduling. The experiment reports
artifact validity, input integrity, obstruction reproduction, cooperative
recovery, catalog attribution, first-eligible-vblank behavior, and the current
product-quality result independently. Reader-policy candidates separately
require an applied policy, capture P95 at or below 250 us, maximum at or below
1 ms, and clean input integrity. A failed latency status is retained as a
successful experimental observation; missing, stale, truncated, or incomplete
evidence is an execution failure. Detailed traces, driver timestamps, and the
independently flushed Main proxy trace are stored under
`build/agent-benchmarks/input-latency-lab/<timestamp>/`. Protocol v3 assigns
each forwarded proxy event an exact sequence and attributes kernel enqueue,
Main poll/read/mapping, proxy journal/write and `EAGAIN`, MagiK proxy capture,
MagiK reader poll return/read start, reader CPU/thread time, applied scheduling
policy, affinity, and poll-interval runqueue delay, mailbox publication/drain,
dispatch, and active-latch confirmation. Main keeps
the bounded trace in memory during the route and performs no hot-path file
output.

The automated uinput route is the causal software-timing authority because it
traverses Main proxy v3 and the production latch path. Protocol-v5 active
sequence confirmation is the presentation authority. Repeated physical
vblanks while the UI is intentionally static during the 600 ms gaps are
reported but are not classified as dropped animation frames.

`settings-navigation` runs the real Home → Settings → About → Info → About →
Settings → Home route first in the normal 1280×720 landscape layout, then in
the monitor-counterclockwise 720×1280 portrait-left layout, at physical
`hdmi-1280x720p60`. The benchmark drives the production input path and waits
for each endpoint's live frame to be physically confirmed before starting the
next leg. Protocol-v5 presentation counters are captured in-process immediately
before the first sliding frame and immediately after the endpoint's physical
confirmation, so idle input, launcher handoff, and host-sampling time are not
part of the cadence window. Every leg independently requires zero protocol-v5 physical dropped frames and ownership
losses, zero latch drops and sequence gaps, continuous hidden-slot
presentation, at least 59.9 physical FPS, whole-frame work P99 below 15,917 µs
and maximum below 16,667 µs, plus no snapshot-lock, locked-Slint-raster, or
status-writer violations. Qualification v4 also reports the synchronous frame
production, render-start, ready-age, post-start, post-request, immediate latch
receipt, completion-poll, and Rust-vsync observations for each leg. These are
diagnostic timing fields; protocol-v5 counters remain the physical cadence
authority.

The matching `settings-navigation-pprof` scenario executes all twelve directed
legs at 999 Hz sampling and retains one route-level flamegraph and folded-stack
file. Its cadence is diagnostic only. Both scenarios override orientation and motion only in
memory, disable catalog refresh, retain confirmed 720p, restore the ordinary
launcher, and verify the exact settings file, retained `MiSTer.ini`, boot
identity, and installed manifest. Evidence is written below the selected
scenario at `build/agent-benchmarks/<scenario>/<timestamp>/`.

Frame-evidence v6 reports workload-neutral `frame_production_*` fields for
event-driven, synchronous-animation, and prepared frames. Screensaver-specific
card and raster measurements retain their `screensaver_*` names; queue depth,
ready age, render time, sequence, starvation, and cancellation do not.

`orientation-transition-fade` and `orientation-transition-zoom` each run the
real Settings view at 1280×720/60 Hz through
Normal → Clockwise → Counterclockwise → Normal → Counterclockwise → Clockwise
→ Normal once with exactly one selected effect. Each endpoint must complete a physical hidden-slot presentation
before the next leg starts. Every leg independently requires zero protocol-v5
physical dropped frames, zero latch drops and sequence gaps, continuous accepted
hidden-slot presentation, at least 59.9 physical FPS, whole-frame work P99 below
15,917 µs, and maximum whole-frame work below 16,667 µs. The scenario changes
orientation and motion state only in memory, disables catalog refresh, retains
only performance evidence, retains the confirmed 1280×720/60 Hz display mode
and its exact `MiSTer.ini`, and restores the ordinary launcher while verifying
the settings hash, boot identity, and installed manifest.
Evidence is written below the selected scenario at
`build/agent-benchmarks/<scenario>/<timestamp>/`.

`orientation-transition-fade-pprof` and `orientation-transition-zoom-pprof`
run the same isolated six-leg workloads with bounded pprof sampling. These
instrumented scenarios provide attribution only and do not qualify cadence.
They retain the confirmed 1280×720/60 Hz mode and restore the ordinary launcher.

New benchmarks must add a named registry entry and a fixed typed device
request. They may not expose arbitrary commands, duration knobs, remote paths,
or generic environment overrides. Benchmark requests pass through a restricted
client that rejects delivery and platform-mutation operations before transport.
The device agent exposes no binary-only runtime replacement endpoint.

## Cold boot

`cold-boot` profiles exactly one supervised reboot of the coherently installed
Dev platform. Before issuing it, the workflow re-verifies the complete
installed platform, rejects every persistent or volatile reset-fault arming
file, rejects a reboot-unstable marker, and syncs. The reboot request is never
retried after an unavailable response.

The device-monotonic timeline starts at Linux boot and records initial Main
entry, final latch Main entry, launcher preflight begin/end, direct Bash exec,
MagiK process entry, every `startup_timing` milestone, and the first real
presented launcher frame. The presentation event uses Main's absolute boot
clock at its native 10 ms resolution; MagiK's finer internal startup clock is
retained separately and is never added to the process-entry timestamp. It also
records host reboot/recovery polling
separately; host timing is not substituted for device-visible startup time.
The retained agent timeline, kernel log, inittab, boot analytics and init-time
diagnostics expose the otherwise uninstrumented Linux/stock-Main interval
without replacing the stock boot executable.
The benchmark requires a new boot ID, active launcher readiness, exact installed
Main revision/hash, and a nonblank authoritative RGB565 capture. Raw Main
events, MagiK log, both status files, the manifest, timeline, summary, capture,
and capture metadata are retained under
`build/agent-benchmarks/cold-boot/<timestamp>/`.

## Arcade launch and return

`launch-return` profiles the coherently installed Dev runtime. Canonical
`release-device` delivery includes the dormant `profile` feature and retains
function symbols while omitting line-level debug sections; the profiler is
activated only by the benchmark's one-shot return environment. A
`release-device-profile` build with full debug information may be produced by
the separate `scripts/agent build runtime-analysis` intent for offline analysis,
but a benchmark cannot invoke or install it or any other temporary runtime.

The workflow selects a deep settled Arcade row (index 128, clamped when the
catalog is shorter), launches a real core, returns twice, and moves one row
between cycles. It records device-monotonic request, acknowledgement,
process-start, exact-context, preview-ready, and first-correct-present
timestamps. Zero or unordered timestamps fail the run rather than producing an
uptime-derived latency. Each cycle
must restore the exact collection, game path/index, visual/scroll position,
and preview state without presenting Home first. It also requires a nonblank
authoritative RGB565 capture, flamegraph, folded stacks, frame profile,
timeline, launcher log, Main events, and pre-launch/restored state.

The v3 summary binds the installed Main revision and hash and reports
command-to-process, process-to-context,
context-to-preview, preview-to-present, and total return latency in
microseconds, plus min/median/max aggregation. Device-monotonic request to
first-correct-present is the authoritative visible-black interval. Host polling
elapsed time is recorded separately and is never counted as visible black. The
explicit restoration/fallback boundary is five seconds. Its timing class is
`instrumented-installed-dev-symbols`: sampling overhead is present, but the
binary and manifest remain the exact installed pair for the entire run.

The former showcase, firework, commercial-technique and Form-scene scenarios
are archived with their code and visual contracts under
`docs/experiments/particles/`. They are not valid production benchmarks.

## First-run intro qualification

The first-run launcher intro is not qualified by a host render average. Device
evidence must begin with both Catalog V3 and the retained Arcade bootstrap
absent, use the production direct hidden-slot route, and retain the complete
launcher frame trace. The qualification run injects no launcher input; UI
responsiveness under navigation load is measured separately so it cannot alter
startup cadence or catalog completion time. A pass derives the required ordered
external-direct post count as `ceil(20 seconds / measured refresh period)`, plus
any reported spinning-cabinet wait frames before the live launcher is ready.
This applies to HDMI and to `crt-240p60`, `crt-288p50`, `crt-480p60`, and
`crt-576p50`. All must sustain the resolved physical refresh within the ordinary
tolerance and zero FPGA physical dropped frames, with latch-protocol drops and
completion failures gated independently. Pacing and completion timing remain
diagnostic. Catalog coordinator and walker
affinity must remain on CPU0. The run also requires a snapshot milestone before
the launcher morph begins and a pixel-identical 20-second frame/cache handoff.
A qualification record includes the resolved route, native framebuffer
geometry, particle density, logical elapsed duration, and expected/captured
frame counts.
A second launcher start must use either the retained Arcade projection or
completed registry and must emit no intro-start event.

Terminology is normative throughout this repository:

- `latch_drop_count` measures rejected or superseded latch protocol posts. It
  says nothing about whether rendering supplied a new frame for every refresh.
- `dropped_frames` is the FPGA protocol-v5 repeated-owned-vblank delta: refresh
  intervals where the MagiK scanout route retained the previous framebuffer.
  It is authoritative at the FPGA scanout source and must be exactly zero
  during an authoritative animation window. Host completion estimates and
  `FBIO_WAITFORVSYNC` observations are diagnostic only.
- `cadence-warning` and `cadence-overrun` describe frame wall-time budget
  observations. They are not latch drops.

Never report zero latch drops as zero dropped frames. A nonzero dropped-frame
count is always a qualification failure; FPS tolerance or healthy latch counters
cannot compensate for it. First-run qualification must show the cadence and
latch-protocol sections as separate gates even when both pass. Sampled profiles
provide attribution only and cannot qualify cadence.

This is runtime/platform qualification, so it requires a clean committed Dev
delivery before measurement. Host tests cover route-aware cue boundaries,
50/60 Hz completion and wait behavior, per-slot zero-write hold behavior,
incremental versus fresh
crossfade equivalence, and pixel equality at the endpoint; they are necessary
but not a substitute for physical latch evidence.

## Fixed particle optimisation trial

`particle-demo-40k` is the campaign comparison trial: exactly 40,960 Visual
particles for 15 seconds at 960x540 RGB565. It covers every deterministic
particle phase, uses the same strict physical-refresh, latch-continuity, render
reserve, cleanup, and restoration checks as the ceiling search, and records
per-phase simulation, projection, clear, raster, prepared-frame age, process
CPU, render P99, and maximum timings. Evidence is written below
`build/agent-benchmarks/particle-demo-40k/<timestamp>/`.

## Particle capacity

The `particles` scenario is the fixed capacity search for the experimental
960x540 scalar particle renderer. It transactionally selects
`hdmi-1920x1080p60`, verifies the resulting 960x540 RGB565 framebuffer, and
requires the direct FPGA vblank-latch hidden-slot backend. The original display
mode and exact `MiSTer.ini` contents are restored even when profiling fails.

For each of the `capacity` and `visual` presets, the search starts at the last
complete-confirmation pass (141,312 particles). It probes only successive
+1,024 counts and stops after three consecutive failures. These probes last
two seconds and intentionally do not require seeing every animation phase.
Only the highest passing count is accepted, and it must still pass a separate
30-second confirmation covering the complete deterministic ten-second cycle.

A count passes only when unique physical latch flips match refresh within
0.1 FPS, P99 render wall time is below the refresh period minus 750
microseconds, and there are no dropped frames, completion gaps, latch
drops, presentation misses or errors, starvation, reused frames, or superseded
frames. Evidence includes per-phase simulation, clear, raster, and render-wall
timings, CPU use, visible counts, and the 32-byte simulation footprint per
particle.

The workflow captures representative static and formed frames through the
typed agent framebuffer path. Telemetry, captures, `summary.json`, and
`report.md` are written below
`build/agent-benchmarks/particles/<timestamp>/`.

## Particle CPU profile

The fixed `particle-profile` scenario samples the scalar particle renderer at
99 Hz for 30 seconds in each preset, using 12,288 capacity particles and 9,216
visual particles. It reuses the screensaver-triggered pprof runtime so sampling
starts only after the particle renderer becomes active. The workflow requires
the direct 960x540 RGB565 hidden-slot path, writes SVG flamegraphs, folded
stacks, metadata, and telemetry below
`build/agent-benchmarks/particle-profile/<timestamp>/`, then restores the
original display mode, INI, launcher environment, and healthy Home screen.

This is attribution evidence rather than a capacity qualification: sampling
overhead may create frame misses. Production ceilings remain owned by the
ordinary `particles` scenario.

## Persisted search

The dedicated `search` scenario first runs a short, read-only benchmark of the
active `arcade` system shard. Four representative queries each record a first
result, one warm-up, and 20 measured iterations. Evidence separates Rust query
preparation, SQLite FTS5 execution, Rust result finalization, and total latency,
with warm p50, p95, and maximum timings.

It then starts the launcher in Arcade with a bounded input script, opens Search,
types `A`, and requires runtime status to report that exact query as `ready`
with at least one result. The ordinary launcher is restored whether verification
passes or fails. This scenario does not start the screensaver.

## Catalog lifecycle

```text
Verify installed platform, health, and exact clean revision
-> create a fixed isolated /tmp catalog root
-> restart the ordinary launcher with one-shot isolated catalog paths
-> keep deterministic controller navigation active throughout the build
-> require first-visible Arcade data within 60 seconds
-> require a valid complete manifest within 20 minutes
-> inspect the registry and every system shard
-> record first-visible/full-build timings, affinity, progress, and per-system counts
-> restart the ordinary launcher without the one-shot environment
-> remove the isolated fixture
-> verify platform identity and health
```

The scenario redirects the sharded catalog, library database, arcade bootstrap
index, ready snapshot, and builder/refresh locks beneath
`/tmp/mister-magik/catalog-lifecycle-benchmark`. Production catalog and library
artifacts are never renamed, deleted, or overwritten. The scripted input lasts
longer than the completion deadline so interaction-dependent catalog starvation
is deterministic. Cleanup and ordinary launcher restart run after every
post-fixture success or failure.

Evidence is written under
`build/agent-benchmarks/catalog-lifecycle/<timestamp>/` as the lifecycle and
diagnostic log, final launcher status, catalog inspection TSV, structured
summary, and Markdown report.

## Catalog PMU attribution

`pmu-profile` is the fixed Cortex-A9 hardware-counter attribution suite. Its v2
report requires the `probe`, `screensaver`, `search`, and `catalog` workloads
from the exact coherently installed Dev runtime. Sampling is fixed at every
span with a 4,096-record per-thread limit. Missing counters, PMU open/read
failures, dropped spans, dropped thread profiles, empty profiles, an installed
manifest change, or failed cleanup invalidate the suite.

The catalog workload owns only
`/tmp/mister-magik/pmu-catalog-benchmark`. It reads the normal library sources,
adds one deterministic synthetic SNES source beneath that isolated root, and
redirects every writable catalog path there. It performs, in order:

1. a fresh build;
2. a changed-input rebuild after adding exactly one synthetic SNES game;
3. a rebuild of every published system.

The changed-input operation must rebuild only `snes` and increase the manifest
game count by one. Rebuild-all must rebuild every published system without
changing the post-increment system or game counts. The workload reopens and
fully validates the manifest and every referenced shard after each operation.
It removes the isolated root on success and failure; production catalog and
source paths are never written.

Catalog profiles combine the builder, library-walker, and foreground publisher
threads. Stable phases cover the outer bootstrap, scan, prepare, and persist
work plus filesystem walking, navigation encoding, SQLite schema creation,
game insertion, search-index population, transaction commit, shard validation,
and artifact copy/hash. Evidence is retained under
`build/agent-benchmarks/pmu-profile/<timestamp>/` as per-workload logs and JSON
plus the v2 suite summary.

Derived ratios use the grouped counters from one calling thread and interval:

- IPC is `instructions / cycles`.
- L1D refill ratio is `L1D refills / L1D accesses`.
- Branch-mispredict ratio is `branch mispredicts / branches`.

PMU counters attribute CPU work; wall time remains the optimization outcome.
Filesystem and SQLite waits can dominate elapsed time without accumulating
corresponding calling-thread cycles. The PMU-enabled workload is therefore not
a cadence or final correctness qualification. Two PMU-off `catalog-lifecycle`
runs with zero FPGA physical dropped frames remain the final device gate.

Catalog optimization campaigns use two clean baseline PMU suites. Each
single-hypothesis commit receives one screening run and, only if promising, two
additional clean confirmation runs. A screen requires at least 5% improvement
in its target phase, 2% in the affected operation, supporting PMU movement, no
operation regression above 2%, exact catalog results, and peak RSS within 5%
or 8 MiB. The three-run candidate median must retain those thresholds and each
confirmation must retain at least 3% of the target-phase improvement. A failed
candidate is removed by a new revert commit; published or local experiment
history is never rewritten.

NEON is eligible only for a byte-exact, contiguous integer loop accounting for
at least 10% of fresh or rebuild-all cycles, averaging at least four 128-bit
vectors per call, and projecting at least a 2% whole-operation improvement.
Filesystem traversal, SQLite calls, sequential hashes, serde, Unicode
iteration, and short strings do not qualify by themselves. Any NEON trial must
retain a scalar fallback and inspect the exact delivered ARM release symbol
with `nm` and `objdump`; build flags alone are not evidence that vector machine
instructions exist. Failed code generation or performance requires an explicit
revert commit.

## Installed screensaver profile

The fixed workflow is:

```text
Verify installed platform and health
-> require screensaver-pprof-v1 and a cached catalog
-> transactionally select 1280x720 HDMI and verify a 1280x720 RGB565 framebuffer
-> start the ordinary launcher with a one-shot environment
-> navigate Home -> Settings -> Show Screensaver
-> profile and stream telemetry for 45 seconds
-> restore the ordinary launcher
-> retain the confirmed 1280x720 HDMI mode
-> verify platform identity and health
```

The benchmark confirms `hdmi-1280x720p60` and intentionally leaves that mode
active after the profile. It records the original mode and INI hash, then
uses the confirmed 720p INI as the final-state baseline. Launcher/profile
cleanup remains mandatory and independent of the retained display mode.

The one-shot launcher environment sets `MISTER_CATALOG_REFRESH=off`. The
launcher must report both the disabled refresh policy and an inactive catalog
worker throughout each active profile. The environment removes itself when the
launcher consumes it; bounded host cleanup is still unconditional after
success, failure, interruption, or an incomplete second run.

Each run uses the production composition and latch-backed presentation path.
Navigation frames are excluded. The first three frames for which runtime
telemetry reports `screensaver_active=true` are activation warm-up: their
timings are recorded as startup evidence but never fail the benchmark. A
screensaver may take several frames to become visible without creating a
user-visible defect.

Steady state begins on the fourth active screensaver frame. Versioned agent
telemetry brackets that window with settled protocol-v5 `0x5c` snapshots.
After wrap-safe invariant, ownership, endpoint, and plausibility validation,
the repeated-vblank delta is `dropped_frames` and must be zero. Latch rejection,
completion, sequence, and flip counters remain independent protocol evidence.
Completion gaps, submitted FPS, wall-time overruns, P99, maximum timings, and
the software drop estimate remain diagnostic attribution only.

This distinction is intentional. Do not tighten startup timing because of a
slow first render, asset loading, allocation, profiler startup, or other
one-time activation work. The benchmark exists to prove that an already
running screensaver does not drop frames.

The complete 45-second run remains the correctness gate. Performance comparisons
also report the final 15 seconds separately, after the parade has had roughly
30 seconds to reach its populated state.

## Restoration

Restoration removes the launcher environment, frame-analytics lease, and
temporary remote profile files, then restarts the ordinary launcher. For
launch-return it also removes the one-shot auto-launch gate and return state;
it never changes the installed runtime or manifest. The
workflow fails separately when performance is outside its gates or restoration
cannot prove a clean, healthy device. The confirmed 720p mode and its exact
`MiSTer.ini` contents, device boot ID, and installed manifest must be unchanged
after profiling.

## Evidence

The SVG flamegraph, folded stacks, profile metadata, telemetry stream,
`summary.json`, and `report.md` are written under
`build/agent-benchmarks/screensaver/<timestamp>/`. Evidence records the
installed revision, display route and framebuffer geometry, and GUI, Main,
scanout-module, and latch-RBF hashes. The report includes presentation
continuity, timing outliers, CPU phases, periodic timing, one-second
maintenance cohorts, and raster-position holds. Pure offline report generators under
`scripts/bench/` may analyze existing data; they must not contact or mutate a
MiSTer.
