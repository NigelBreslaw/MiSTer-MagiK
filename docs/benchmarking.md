# Legacy platform benchmarking

For everyday application development, use `scripts/magik2 check` for smoke,
`check idle` for two real-app measurement windows, and `check idle --profile`
for one separate profile. Mini-MagiK uses `--app mini-magik` and the `motion`
scenario. These all run through the same Python scenario framework; see
[development commands](../magik2/README.md).

The specialized legacy workflows below remain separate qualification tools.
Select a scenario explicitly; bare `scripts/agent benchmark` is removed.

The fixed attribution workloads and evidence gates for the cross-subsystem
profiling campaign are defined in
[Deep profiling campaign](deep-profiling-campaign.md). Those instrumented runs
are diagnostic; the unprofiled controls and protocol-v5 presentation evidence
remain performance and cadence authority.

Input loss and latency qualification uses the typed
`scripts/agent benchmark input-integrity` workflow described in
[Unified input](input.md). Do not substitute raw SSH input injection: the gate
must traverse Main's real mapping, aggregation, proxy-v2, and kernel path.
The fixed gate compares a bounded launcher event trace against the exact injected
sequence in idle and catalog/CPU/stall scenarios. Intentional UI stalls are input
correctness stress, not rendering-cadence qualification.

`scripts/agent benchmark SCENARIO` selects a retained legacy qualification
workflow. Scenarios are a closed typed registry rather than a flag matrix. It
never builds, deploys, or replaces platform files. The fixed `cold-boot`
scenario may issue one supervised Linux reboot. The isolated
`arcade-catalog-prototype-cold` scenario may issue three supervised reboots for
fresh indexed-fast, filtered-fast, and filtered-full-walk active builds. All
other scenarios must leave the device boot unchanged. The
installed platform manifest and its hashes are the benchmark identity, and its
delivery reconciliation against the clean local Git HEAD must be a no-op.
Host-only benchmark tooling changes therefore do not force an identical runtime
revision, while pending runtime or platform changes remain a hard failure.

Supported scenarios:

- `screensaver`
- `cold-boot`
- `cold-boot-pprof`
- `input-integrity`
- `launcher-response`
- `launcher-response-retained`
- `launcher-response-attribution`
- `gui-frame-attribution`
- `settled-composition`
- `bridge-model-churn`
- `bridge-model-churn-retained`
- `scheduler-trace`
- `storage-attribution`
- `arcade-velocity-scroll`
- `arcade-velocity-scroll-attribution`
- `system-entry-critical-streamline`
- `transition-streamline`
- `agent-observer-attribution`
- `launcher-response-streamline`
- `input-latency-lab`
- `catalog-lifecycle`
- `catalog-resume-validation`
- `arcade-catalog-prototype-cold`
- `launch-return`
- `launch-return-once`
- `launch-return-fallback`
- `launch-return-attribution`
- `modal-input`
- `navigation-transitions`
- `settings-navigation`
- `settings-navigation-pprof`
- `orientation-transition-fade`
- `orientation-transition-zoom`
- `orientation-transition-fade-pprof`
- `orientation-transition-zoom-pprof`
- `neon-attribution`
- `pmu-profile`
- `media-pack-persistence`
- `rom-identity-hashing`
- `preview-work-attribution`
- `search`
- `streamline`

`rom-identity-hashing` is a read-only exact-device authority for the production
streaming ROM identity implementation. It selects deterministic small, medium,
and large production files for Lynx, NES, SNES, Mega Drive, and N64 where those
classes exist, records I/O, fused transformation/CRC work, lookup, cooperative
checkpoints, faults, RSS/HWM, CPU placement, and PMU attribution, and hashes
every candidate and result. Lynx
is reported separately because it is the only production-default full-ROM hash
policy; all other systems are opportunity context and cannot justify promotion
on their own. Candidate discovery uses the authoritative software-hash cache and
the five target system directories; it must not traverse the complete library
namespace. Three isolated production runs provide comparable RSS/HWM and must
agree on all candidate CRC, identity, family, rank, and software-cache hashes.
Duplicate production-path parity and PMU attribution are bounded to the small
production-default Lynx case so transformed opt-in ROMs cannot exhaust the
typed operation timeout. The retired whole-file/streaming selector is not
available. The scenario neither starts catalog work nor enables forced
background refresh.

`media-pack-persistence` is the read-only exact-device authority for the raw
`.mmlz4b` download/save flow. It selects the small, median, and largest pack for
the configured production image size, primes the remote cache, and runs three
isolated production controls through `media-bench-download`. Benchmark artifacts
use hidden, timestamped paths and are removed after every arm; authoritative
pack, index, and media-state files are never replaced. The report records raw
network/tmpfs/exFAT bytes, coupled network-and-destination-write time,
verification, save/publication, total flow, throughput, process RSS/HWM, and
that production decode time is zero. Index metadata and single-writer policy
are recorded; the isolated persistence arm deliberately does not download the
sidecar, so index overlap is explicitly reported as unexercised. Production
writes and hashes into a hidden sibling exFAT temporary file, then syncs,
renames, and parent-syncs after verification. The staged/direct selector is
retired. Catalog refresh remains off and is never forced for this scenario.

`catalog-resume-validation` is the exact-device authority for interrupted
initial-build recovery. Each of three samples creates an isolated missing
catalog, waits until the production builder has synced at least one durable
target checkpoint, and then interrupts it with an ordinary Dev launcher
restart. The restarted launcher receives the same isolated catalog contract and
must reuse committed targets, publish a valid exact catalog, and leave the
production registry unchanged. The report separates journal open, compact-frame
decode, validation walk, bounded-channel wait, validation consumer work, and
recovered-output decode, together with namespace I/O and RSS/HWM evidence. It
never uses direct-reset fault injection and does not set the forced-background
catalog option; the absent isolated catalog invokes genuine first-build policy.
Resume validation computes the same target fingerprints on the joined
LibraryWalker without a per-entry channel. The retired event path remains only
in parity assurance so future fingerprint changes must agree with the
production walker-native result.

`preview-work-attribution` is the exact-device opportunity gate for sharing
selected and prefetch preview work. It runs three fresh Arcade system-entry
samples, three ordinary held-scroll samples, and three turbo-scroll samples.
The arm enables a one-shot trace that records resolved archive, asset, resize,
queue, read, decode, cache, and sidecar activity without changing the two-worker
production implementation. An experiment is authorized only when duplicate
work is at least 2% of measured preview work or collisions cover at least 5% of
requests. System-entry selected-preview p95 must remain at or below 85 ms.
Catalog refresh is always off and is never forced by this scenario.

`search` runs three complete `pac`, `street`, `capcom`, and `2 player` suites
against one validated catalog generation, then qualifies one request through
the production launcher UI path. Exact system IDs, ordinals, rank bits,
and autocomplete contents are hashed for every query. The timing report
separates SQLite open, statement preparation, execution, and Rust work and
records opens, prepares, faults, RSS, and HWM. The UI prerequisite is established
by a fresh launcher with catalog refresh off; the scenario never enables forced
background catalog work. The UI report records whether the request used the
resident catalog projection or created a persisted-search worker; zero worker
creation is valid when the resident projection owns the result.

The Arcade velocity-scroll profiling scenarios default to the active display
route. A typed arm also accepts `--route active`, `--route hdmi-landscape`,
`--route hdmi-portrait-left`, `--route hdmi-portrait-right`,
`--route hdmi1080-landscape`, `--route hdmi1080-portrait-left`,
`--route crt240-portrait-left`, `--route crt240-portrait-right`,
`--route crt288-portrait-left`, or `--route crt288-portrait-right`. Explicit routes select the display
transactionally and apply orientation only to the benchmark launcher's
in-memory state. On success or failure the runner restores and verifies the
exact original display mode, `MiSTer.ini`, settings file, launcher environment,
boot identity, installed manifest, and launcher health. Every artifact records
the requested route plus the effective display mode and orientation. The
unprofiled `arcade-velocity-scroll` run owns
physical cadence and repeated-vblank qualification. The single
`arcade-velocity-scroll-attribution` scenario runs the control, pprof, PMU,
and Streamline arms sequentially and writes one correlation manifest. A typed
optional arm (`control`, `turbo`, `pprof`, `pmu`, or `streamline`) runs
only that fixed arm, for example `scripts/agent benchmark
arcade-velocity-scroll-attribution pprof --route hdmi-portrait-left`. Every arm starts on Home with Arcade
preselected and enters it with one confirmation; the former Settings-to-Home
navigation and focus-panning preamble is not part of the workload. The
unprofiled control, turbo, and profiler arms use a fixed 40-second hold;
profiler arms remain attribution-only. The `turbo` arm primes the production
turbo gesture with a quick Down tap, then holds Down at the 720 px/s turbo
speed; it does not use the synthetic benchmark-only bounce helper.
All Arcade arms use the full 40-second workload. The control and turbo arms
own cadence qualification; profiler arms remain attribution-only.

The optional `--duration-seconds N` argument is accepted only by the single-arm
`arcade-velocity-scroll-attribution` route and is bounded to 5–120 seconds. It
defaults to 40 seconds. Unprofiled runs shorter than 40 seconds are directional
development evidence; only unprofiled runs of at least 40 seconds can qualify
cadence. Every artifact records the requested duration and evidence authority.

Optimization campaigns establish one compatible baseline immediately before a
route/profiler is first changed and compare later artifacts to that baseline;
they do not run the aggregate attribution suite between commits. HDMI portrait
qualifies the dense physical Arcade and preview producers, HDMI landscape
qualifies the ring-backed Arcade producer with shared preview publications,
and CRT portrait qualifies complete cached-frame ownership plus its retained
Arcade overlay. PMU and Streamline can attribute work but cannot overrule an
unprofiled control. The host orientation matrix complements those device runs
by checking Home, System Hub, Arcade, search, Settings, dialogs, screensaver,
and transition endpoints across normal/clockwise/counterclockwise HDMI and
240p/288p portrait CRT layouts.

The shared-composition campaign's final device qualification is deliberately
limited to six 40-second `turbo` controls: `hdmi-landscape`,
`hdmi-portrait-left`, `hdmi1080-landscape`, `hdmi1080-portrait-left`,
`crt240-portrait-left`, and `crt288-portrait-left`. This covers both 720p and
1080p HDMI in landscape and portrait while avoiding a redundant second device
run for the opposite portrait rotation. Portrait-right remains a supported
benchmark route and is covered by deterministic host pixel/ownership parity.
Every qualifying control requires settled cadence endpoints, authoritative
FPGA-latched terminal scanout, zero repeated refreshes, latch drops, sequence
gaps, ownership loss, or profiling record loss, and the route-specific FPS and
foreground-work budgets.

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

`settled-composition` is the unprofiled authority for steady modal ownership
and the cache-recovery cost of a full-raster navigation destination. It opens the
real Arcade filter drawer first to capture disjoint Slint chrome damage while
custom layers remain active, then opens the favorite confirmation without
accepting it and waits for eight
frames after direct-layer retirement has a matching physical receipt, then
cancels the dialog and drives Home → Settings. The destination frame and its
immediately following frame are reported separately with composition state,
retirement generation/receipt, full-present reason, Slint damage and raster
time, custom-layer work, latch-copy bytes, and protocol-v5 cadence. Because the
immediate physical frame can present without invoking Slint, capture continues
until the first real ordinary Slint raster in settled Settings. The summary
also reports every forced-full raster through its first subsequent real Slint
render, resetting the window if another forced raster supersedes it. An
authoritative terminal Settings PNG and its hash support pixel-parity checks.
The route restores the original Home selection and display mode, and leaves
favorite state unchanged. Evidence is retained below
`build/agent-benchmarks/settled-composition/<timestamp>/`.

Production full rasters retain Slint's partial-render cache while repainting the
complete reusable RGB565 backing buffer. The summary reports this as the
`reused-buffer` policy. NewBuffer remains an explicit fallback only for a newly
allocated or otherwise discontinuous backing store. Qualification uses
`full_raster_recovery.combined_raster_us` and requires identical terminal pixels,
zero duplicate full recovery rasters, and unchanged cadence.

`bridge-model-churn` is the unprofiled exact-device authority for repeated
production bridge updates. A consumed laboratory route publishes 60 fixed
media-progress events through `MediaProgressDisplay` and the worker UI intent,
selects 64 rows in a retained 128-row production Slint model, then requests 64
unchanged light bridge revisions. Every update waits for physical presentation.
The profile reports model replacements, row mutations and allocations,
`SharedString` constructions, model-allocation time, bridge time, Slint raster,
damage, copied bytes, and cadence per phase. It verifies terminal media summary
and row contents, selection/accessibility state, then republishes the real menu
and clears benchmark media before completing. The original semantic Home state,
display mode, installed manifest, boot identity, and ordinary launcher are
restored. Evidence is retained below
`build/agent-benchmarks/bridge-model-churn/<timestamp>/`.
Retained bridge models are the sole production policy. The launcher retains the
media `VecModel`, coalesces only nonterminal progress to 100 ms, publishes
completion/failure immediately, updates only previous/current menu rows when
feedback identity is stable, and avoids `SharedString` construction for
unchanged bridge values. `bridge-model-churn-retained` and
`launcher-response-retained` remain compatibility aliases for historical
automation; they execute the same production path as `bridge-model-churn` and
`launcher-response` and no selector exists.

Production modal carriers are receipt-scoped: the entry carrier remains forced
until direct-layer retirement has a matching physical receipt. The summary
separates any required invalid-slot convergence copy from recurring steady
modal work while retaining the total retirement-confirmed byte count.


`navigation-transitions` runs the fixed scripted route Home → Arcade → Home →
Consoles → System → Consoles → Home with the Super-Scaler POC enabled. It
records per-frame process CPU, transition and overlay cost, snapshot locking,
Slint calls, status-writer overlap, preparation attribution, latch sequence
continuity, physical FPS, and drop counters for all six legs, then restores the
ordinary launcher. Evidence is written below
`build/agent-benchmarks/navigation-transitions/<timestamp>/`.

`launcher-response` is the release gate for interactive selection latency and
confirmed focus feedback. Schema v2 has no v1 compatibility parser. It runs
with the production-default catalog policy: an existing catalog is loaded but
no source reconciliation is requested. It drives eight independent Computers
focus changes by alternating Acorn and Apple II, using a 100 ms baseline plus
rotated 50/57/64/71 ms start-to-start schedules. It also covers discrete System
Hub and Settings focus and Arcade press-to-first-motion through supported Main
proxy protocol v2 or sequence-tagged laboratory protocol v3.

Each eligible destination must have one exact pulse-on and pulse-off active
latch sequence with at least 80,000 µs between their physical confirmations.
Pulses may overlap; the final focus change must confirm without waiting for an
older pulse deadline. Arcade's velocity list deliberately has no pulse and
is gated on first confirmed motion instead. Runs execute the normal warm-launch
state at physical 60 Hz and 50 Hz, with zero input loss, duplication,
coalescing, reorder, overflow, desync, latch drops, or ownership loss. Every
response and feedback frame must become active on its first eligible vblank.
Repeated vblanks while the UI is intentionally static remain reported but are
not classified as input loss or dropped response frames.

Dispatch P95 must be at most 3 ms and its maximum at most 5 ms. Input-to-visible
median must be at most 12 ms; each display-rate leg independently limits P95 to
one refresh period plus 3 ms and the maximum to one refresh period plus 8 ms.
The report exposes independent input-response, pulse, and integrity statuses.
Background adoption is not applicable to this steady-state qualification and
must not be forced by `launcher-response` or `launcher-response-retained`.
Their overall result is authoritative only for the ordinary interactive state.

Forced catalog reconciliation is exceptional-path evidence, not a release gate
for ordinary interaction. A missing catalog may build on first boot, and a user
may explicitly choose Settings → **Refresh Database**; those paths retain their
own correctness and responsiveness coverage. Forced catalog work may be used
only by an explicitly named stress or attribution arm such as
`input-latency-lab`. Its timing, cadence, and feedback results are reported
separately and must be compared for candidate regressions, but they do not veto
an otherwise passing steady-state optimization. Do not set
`MISTER_CATALOG_REFRESH=force` automatically when adding or extending a routine
UI qualification.

For attended diagnosis of intermittent single-press latency, set
`MISTER_LAUNCHER_RESPONSE_ISOLATED_PROFILE=1` when running `launcher-response`.
This diagnostic mode is not the release qualification: it holds the physical
display at 1920×1200p60, keeps background catalog refresh off to match the
production default, enters
Computers independently for 200, 300, 400, and 600 ms start-to-start schedules.
Each schedule runs Acorn→Other→Acorn twice and records every capture, dispatch,
and exact active-latch confirmation. The ordinary command remains the 60/50 Hz
multi-route qualification above.

Launcher-response trace schema v6 can additionally carry one-shot UI-thread
execution attribution. When enabled by a typed diagnostic workflow, drain,
dispatch, state projection, raster, latch post, confirmation, and retained
scheduler intervals include thread CPU time, voluntary and involuntary context
switch deltas, and CPU migration. These stamps classify wall-time gaps as
on-CPU work, voluntary waiting, preemption, mixed delay, or unresolved off-CPU
time. They are disabled for ordinary qualification because the required Linux
syscalls would otherwise perturb the latency being qualified.

The dormant `launcher-response` pprof trigger starts only after the exact
Computers/Acorn frame becomes the active latch sequence. It stops after the
route's final acknowledgement-removal frame is physically confirmed and the
completed response trace has been handed to its writer. Profile finalization is
therefore outside the measured response window. The installed runtime advertises
this support as `launcher-response-pprof-v1`; typed attribution workflows own
the volatile trigger and artifact paths.

The matching PMU arm is independently one-shot. After Acorn confirmation it
samples only input-bearing routing, interaction projection, Slint raster,
damage/frame planning, hidden presentation, post-confirmation, and frame-tail
spans. The final active span is closed before the bounded thread profile is
removed from memory and written after response-trace completion. Its
`mister-magik-launcher-response-pmu-v1` artifact separates processor work from
the execution-stamp arm's off-CPU wall time; PMU counters are never treated as
evidence about time when the UI thread was not running.

`launcher-response-streamline` records the same 1920×1200p60, catalog-off
200/300/400/600 ms Computers round trips inside one bounded system-wide Arm
Streamline capture. System-wide collection is intentional: the measured path
crosses Main's input proxy, the launcher UI thread, kernel scheduling, and the
latch driver, and the launcher is restarted between schedules. The capture uses
the low sampling rate, includes kernel execution, disables stack unwinding, and
has a 120-second hard limit. APC artifact validity and input integrity are
reported separately from the existing product latency gates, so a valid capture
is retained even when the latency result fails.

`launcher-response-attribution` is the fixed diagnostic suite used before a
production scheduling or rendering change. It independently restarts and runs
the four exact round-trip schedules under a zero-observer control, execution
stamps, 997 Hz pprof, and sampled per-thread PMU counters, then runs the required
system-wide Streamline pass. Each arm retains its complete traces and native
artifacts below an arm-specific directory. The report compares observer latency
against control and separates artifact validity, input integrity, and current
product-quality status. It therefore completes successfully when all evidence is
valid even if the control still misses the product latency target. The command
requires an explicit audited `MISTER_GATORD_PATH`; it never silently skips the
system timeline.

`gui-frame-attribution` independently restarts the fixed 1280x720p60 route for
an unprofiled control, a dormant-window PMU arm, and a bounded system-wide
Streamline arm. Each route uses authenticated production input from settled
Settings through Home pan right/left and held Arcade scroll to a terminal
preview and confirmed settled Arcade frame. Artifact validity is independent
from control-arm product quality; every arm restores the ordinary launcher and
the scenario restores the original confirmed display mode.

`scheduler-trace` runs that same fixed 1280x720p60 GUI route inside an isolated,
bounded native tracefs instance. It records scheduler switches, wakeups,
migrations, IRQs, and softirqs without modifying the global tracer or requiring
an uploaded target binary. The summary attributes on-CPU time, runnable delay,
preemption, CPU placement, dual-core overlap, and interrupt cost. Buffer
overruns, missing core events, identity drift, or incomplete cleanup invalidate
the artifact. The trace remains diagnostic-only; the unprofiled route owns
product-quality conclusions.

`storage-attribution` runs the production `library-refresh` workload against
the normal configured sources while redirecting every writable catalog path to
the fixed Dev-only `storage-attribution-benchmark` directory on `/media/fat`.
It combines process-I/O samples, backing-MMC block statistics, block tracepoints,
and available metadata/sync syscall tracepoints. The workflow caps the isolated
output at 512 MiB and 20 minutes, validates the generated catalog, removes the
exact benchmark root on every path, and proves that the installed identity and
production catalog registry did not change. It is diagnostic attribution, not
a storage or product-quality gate.

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
forced-catalog arms compare the production CPU1/nice -15 input reader against
the same CPU1 policy applied as a laboratory override and low-priority
round-robin scheduling on CPU0 and CPU1. These
reader policies require the same consumed volatile lab token and do not alter
ordinary runtime scheduling. Per-event `/proc` scheduler accounting is enabled
only for the current-policy baseline/forced attribution pair; candidate timing
arms retain the non-perturbing poll, CPU, and thread-clock stamps. The
experiment reports artifact validity, input integrity, obstruction
reproduction, cooperative recovery, catalog attribution,
first-eligible-vblank behavior, steady-state product quality, and
forced-catalog stress quality independently. Forced arms are diagnostic
comparisons and do not contribute to steady-state product qualification.
Reader-policy candidates separately
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

The portrait half of this route exercises native physical-space composition:
Slint software-renderer rotation, mapped Arcade and preview rectangles, and
physical navigation snapshots/effects. `orientation_damage_rotation_us` must
remain zero. A nonzero value indicates a regression to post-raster portrait
rotation even when cadence still passes.

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

`transition-streamline` runs the fixed landscape/portrait Settings-navigation
route followed by the fixed fade and zoom orientation routes inside one bounded
system-wide capture. Every route has an independent device-monotonic bracket,
and the summary keeps landscape, portrait, fade, and zoom evidence separate
while reporting the combined snapshot/raster, portrait composition,
hidden-slot copy, post, and confirmation dimensions. A complete identity-bound
capture remains a valid attribution artifact even when observer overhead makes
an existing product gate fail; the unprofiled scenarios above remain cadence
authority.

`agent-observer-attribution` runs the same production Home pan right/left for
no observer; telemetry at 1000 ms and 100 ms with analytics off and process;
and adaptive and full framebuffer streams. It repeats the complete seven-arm
matrix inside one system-wide capture. The v1 summary separates Home
presentation deltas and latch health from agent phase wall cost, transport
volume, and Streamline-owned CPU attribution. Every arm owns a fresh launcher
and restores the normal launcher before the next arm.

New benchmarks must add a named registry entry and a fixed typed device
request. They may not expose arbitrary commands, duration knobs, remote paths,
or generic environment overrides. Benchmark requests pass through a restricted
client that rejects delivery and platform-mutation operations before transport.
The device agent exposes no binary-only runtime replacement endpoint.

## System entry

`scripts/agent benchmark system-entry` measures every populated fast-catalog
system. Each system gets a fresh launcher process containing registry summaries
and a generation-bound entry reader; merely focusing the destination tile
performs no shard load or prelude fault. The benchmark sends one production-path activation press, then
records row readiness, the first presented list frame, selected-preview
readiness, and the first Main-confirmed active frame containing both the full
list and the terminal selected screenshot state. It retains that frame as a
PNG together with capture metadata and the event trace. The benchmark enters
the game list directly; production SNES activation first presents the System
Hub containing Games, Recent, and Favorites.

The discovery sweep samples every populated system once. The three slowest
systems then receive two additional samples, and `summary.json` identifies the
worst confirmed system by median complete-ready latency. Samples are process
cold: the launcher is restarted for every sample, but the operating-system
filesystem cache is deliberately not flushed. Screenshot capture starts only
after the ready marker and after the asynchronous launcher status confirms the
same active system list; both the status wait and capture are excluded from the
latency.

A system that does not reach the authoritative ready marker is retained as a
failed sample with its partial trace, final launcher status, and any available
framebuffer capture. Discovery continues so every populated registry system
has an initial result. Failed samples are excluded from slowest-candidate
selection, listed in `unready_systems`, and make the summary fail without
naming a misleading worst system.

`scripts/agent benchmark system-entry-critical` is the short regression loop
for C64, SNES, PC-88, NES, BBC Micro, and Arcade. It runs each system once in a
fresh launcher process. The runtime directly invokes the same collection-entry
helper used by production activation after startup input readiness; it does not
focus a tile, traverse menus, or synthesize controller input. A fixed two-second
Home settle after input readiness prevents startup work from contaminating the
sample and is outside the measured interval. Timing begins at the collection
shard request and retains the same Main-confirmed list and terminal-preview
boundary, trace, status, and screenshot artifacts as the full sweep. Run the
full `system-entry` benchmark only when the critical set needs to be
rediscovered.

`scripts/agent benchmark system-entry-critical-confirm` repeats that same
direct measurement 10 times per critical system and reports nearest-rank P95,
median, and maximum latency for every stage. `scripts/agent benchmark
system-entry-qualification` applies the 10-process contract to every populated
registry system in registry order. Both modes retain partial failed samples
instead of dropping them from the artifact set or treating them as slow
successful samples.

Each critical-system summary row exposes four cumulative timings: all
registered game rows loaded, first visible list frame, selected-game screenshot
terminal state, and complete Main-confirmed readiness. The run fails if the
loaded row count differs from the registry count. When the selected game has a
screenshot, readiness requires the exact screenshot; otherwise it requires a
confirmed empty preview. The retained authoritative framebuffer capture is
taken only after that ready frame, and its image, capture sequence, and metadata
are linked directly from the system row.
Every sample also records whether the catalog was already resident when entry
began and the provenance of the terminal preview (for example decoded cache,
archive, or authoritative empty). This makes startup-only advantages such as
Arcade residency visible in both screening and confirmation reports.

The direct modes intentionally contain no fullscreen animation. Their
protocol-v5 source-to-ready observation therefore reports repeated vblanks as
diagnostic static-source intervals and never labels them dropped frames; latch
drops remain a hard failure. Actual animation cadence is qualified separately
by a scenario with an exact transition-start-to-confirmed-endpoint window, such
as `settings-navigation`. Static repeated vblanks and animation dropped frames
must not be combined.

`scripts/agent benchmark system-entry-critical-profile` is attribution-only.
It runs isolated C64 and SNES direct entries with the dormant installed-runtime
pprof sampler and per-thread Cortex-A9 PMU spans enabled. The retained evidence
splits descriptor lookup, NavPack open/mmap/header/first-viewport work,
collection publication and CPU1 adoption. It also records worker
wall/thread CPU time, CPU identity, page faults and measured allocations, plus
selected-preview I/O/decode timing and
destination-frame preparation.
Profiled samples never determine a performance pass; the unprofiled screening
and confirmation commands remain authoritative.

`scripts/agent benchmark system-entry-critical-streamline` runs the same fixed
direct C64 and SNES entries inside one bounded system-wide Streamline capture.
Each entry retains its own device-monotonic start and end bracket together with
the existing descriptor, NavPack, row projection, catalog publication, preview,
and CPU1-adoption evidence. The APC, archive, symbols, capture identity, launcher
log, and per-system route artifacts are one attribution set. Its artifact status
depends on completeness and identity, not latency; unprofiled system-entry
controls remain the performance authority.

The ready trace binds the active catalog version, preview presentation
generation and Main sequence to the confirmed list frame. Capture remains
outside the latency interval and follows that generation-bound confirmation.

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

`cold-boot-pprof` runs the same single supervised reboot with a self-removing,
profiler-only launcher environment. It starts the installed runtime's dormant
pprof sampler at MagiK process entry and stops after the first real launcher
frame has enabled application input. The retained flamegraph and folded stacks
attribute MagiK CPU work only; the ordinary `cold-boot` scenario remains the
timing authority because sampling perturbs execution and cannot explain the
Linux or Main intervals. Profile evidence is written below
`build/agent-benchmarks/cold-boot-pprof/<timestamp>/`. Cleanup removes the
one-shot environment and volatile profile directory and verifies that no boot
arming file remains.

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

`launch-return-once` is the attended incident-reproduction route. It begins at
Home, enters Arcade through typed semantic automation, selects
`1943 Kai Midway Kaisen (Japan).mra`, launches it once, and returns once. It
never queues a second launch. The route requires the exact Arcade collection,
game, and list index to remain restored after authoritative catalog
reconciliation. It captures the restored Arcade framebuffer and physical USB
Video frame immediately at that boundary, then records passive FPGA
diagnostics and Main/launcher logs without navigating elsewhere. A video-level
black or classified corrupt frame is retained as evidence and fails the run;
cleanup releases only the volatile automation lease and does not repair or
restart the launcher.

`launch-return-attribution` repeats the same fixed two-cycle route in four
independent launcher processes: unprofiled control, PMU, existing pprof, and
system-wide Streamline. Capsule construction/encoding/save is attributed to
the UI thread while launch preparation, archive extraction, FIFO request, and
acknowledgement are retained as a submitted launch-worker profile. Every arm
must restore exact context, terminal preview state, and an authoritative first
correct presentation. The v1 artifact remains valid when a diagnostic arm, or
even the control, exceeds five seconds; the summary reports that product
boundary separately and treats only the unprofiled control as timing authority.

The former showcase, firework, commercial-technique and Form-scene scenarios
are archived with their code and visual contracts under
`docs/experiments/particles/`. They are not valid production benchmarks.

## First-run intro qualification

The first-run launcher intro is not qualified by a host render average. Device
evidence must begin with the fast catalog absent
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

## Catalog discovery architecture

The catalog build uses one serial discovery plan for the configured storage
root. The plan reads installed-core entries and top-level game-directory headers
once, then the source adapters consume those facts while walking each namespace
once. The checked header pass does not issue a metadata probe for every child:
directory signatures and the final game-directory facts come from the serial
namespace traversal that already has the directory open. If a directory type is
uncertain, the existing exact-path fallback remains responsible for that entry.

Prepared sources are also serialized. Arcade is built first because it owns the
initial completion callback. The remaining prepared systems run once in
`PREPARED_SYSTEM_IDS` order with completion callbacks suppressed; their exact,
successful non-empty watch roots are then excluded from generic discovery.
`plan_ready` is emitted before those prepared completions are replayed in the
same order. Generic discovery excludes only those observed `PathBuf` roots and
the existing prepared system IDs. It does not use a second hardcoded root table,
case-folded path matching, or a parallel SD-card worker.

On Linux the normal namespace walk is fd-relative and serial. A typed nested
`ENOENT` can recover by taking a bounded WalkDir snapshot of only the affected
subtree and then continuing the parent walk. A snapshot is usable only when
WalkDir reports no errors, the subtree root is still a directory, and the
existing capture budgets are respected. Partial, missing, oversized, root-level,
non-`ENOENT`, and other nonrecoverable cases retain the whole-root WalkDir
fallback. This keeps incomplete data out of catalog rows and watch fingerprints;
the fallback and recovery records remain separately attributable in benchmark
evidence.

The invariant for every optimization is unchanged catalog rows, launch plans,
logical fingerprints, system/game counts, and rebuild behavior. Filesystem
enumeration, metadata, archive reads, catalog writes, SQLite work, rename,
sync, delivery, and benchmark invocations remain serial because the SD card is
the limiting shared resource. The control timing is the Rust benchmark entry
implemented by `scripts/agent benchmark catalog-attribution-control`, not a
standalone script.

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

Set `MISTER_PMU_COUNTER_SET=cortex-a9-neon` to replace the general group with
the non-multiplexed Cortex-A9 attribution group: cycles, speculative
instructions (`0x68`), NEON instructions (`0x74`), NEON-clock-active cycles
(`0x8c`), data-dependent stall cycles (`0x61`), L1D accesses, and L1D refills.
The v2 thread profile records the selected set and serializes only counters
that the kernel actually returned. Its per-span derived data adds:

- speculative IPC (`speculative instructions / cycles`);
- NEON instruction share (`NEON instructions / speculative instructions`);
- NEON throughput while active (`NEON instructions / NEON-clock-active cycles`);
- NEON clock duty (`NEON-clock-active cycles / cycles`);
- data-dependent stall ratio (`data-dependent stall cycles / cycles`);
- L1D refill ratio (`L1D refills / L1D accesses`).

PMU counters attribute CPU work; wall time remains the optimization outcome.
Filesystem and SQLite waits can dominate elapsed time without accumulating
corresponding calling-thread cycles. The PMU-enabled workload is therefore not
a cadence or final correctness qualification. Two PMU-off `catalog-lifecycle`
runs with zero FPGA physical dropped frames remain the final device gate.

`scripts/agent benchmark neon-attribution` is the fixed cross-path NEON
campaign. It runs the probe, screensaver, search, and catalog span suite with
`cortex-a9-neon`, then runs the Arcade velocity-scroll GUI profile in normal
landscape and monitor-counterclockwise portrait modes. The two GUI legs expose
the compositor, list rotation, overlay copy, blend, latch-copy, and presentation
spans that surround the production RGB565/NEON routes. Every nested profile must
declare `cortex-a9-neon`; mixed or missing counter-set provenance invalidates the
campaign. Evidence is retained under
`build/agent-benchmarks/neon-attribution/<timestamp>/` with separate runtime,
landscape, and portrait artifacts plus the combined v1 summary.

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
