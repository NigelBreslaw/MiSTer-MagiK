# Deep profiling campaign

This campaign turns the existing launcher-response attribution pattern into a
decision-grade profiling surface for MiSTer MagiK's GUI/runtime and device
agent. It collects attribution evidence only. Performance changes are proposed
after the campaign and are implemented separately.

## Scope and priority

The first campaign investigates these workloads in order:

1. GUI frame production: bridge synchronization, Slint raster, custom RGB565
   composition, hidden-slot copy, publication, and latch confirmation.
2. Observer interference: telemetry and framebuffer observers competing with
   the launcher during the same fixed Home-pan workload.
3. System entry: the existing direct C64 and SNES critical-entry routes.
4. Navigation and orientation transitions: Settings navigation plus fade and
   zoom orientation effects in landscape and portrait.
5. Launch and return: return-capsule work, preparation and Main handoff across
   two complete launch/return cycles.
6. Device-agent I/O: telemetry, framebuffer capture, library snapshots, and
   large allowlisted directory listings.

Screensaver, particles, and video are excluded from this first campaign.
Screensaver and particles already have focused profiling coverage. Continuous
video is currently a lower expected opportunity than interactive frame
production and the observer and I/O paths above.

## Fixed workload matrix

All scenarios use the exact coherently installed Dev platform. They expose no
arbitrary command, environment, path, or duration parameters.

| Scenario | Control and instrumented arms | Fixed workload | Retained evidence |
| --- | --- | --- | --- |
| `arcade-velocity-scroll-attribution` | unprofiled control, 999 Hz pprof, per-frame Cortex-A9 PMU, and system-wide Streamline arms | Home with Arcade preselected, one confirmation into settled Arcade, then screenshot backdrop changes on the active route, including CRT 240p; fixed 40-second holds | control cadence summary, flamegraph and folded stacks, per-span PMU JSON, APC/archive, shared correlation manifest |
| `gui-frame-attribution` | independent control, PMU, and system-wide Streamline arms | settled Settings; Home pan right and left; held Arcade scroll with preview changes; settled Arcade at confirmed 1280x720p60 | per-frame TSV, PMU JSON, APC and archive, launcher log, status and presentation snapshots |
| `system-entry-critical-streamline` | one bounded system-wide Streamline arm | existing direct C64 and SNES entry routes with separate monotonic windows | APC and archive, phase evidence, catalog and presentation snapshots |
| `transition-streamline` | one bounded system-wide Streamline arm | existing Settings route plus fade and zoom orientation routes in landscape and portrait | APC and archive, per-effect and per-orientation frame evidence |
| `launch-return-attribution` | independent control, PMU, pprof, and system-wide Streamline arms | fixed two-cycle launch/return route | timing, PMU and pprof artifacts, APC and archive, context and first-correct-present evidence |
| `agent-observer-attribution` | no observer; telemetry at 1000 ms and 100 ms with analytics off and process; adaptive and full framebuffer streams; then the same matrix in one system-wide capture | identical fixed Home-pan route for every arm | GUI frame evidence, agent phase timing, observer deltas, APC and archive |
| `agent-io-attribution` | one fixed repeated operation sequence and system-wide capture | raw and PNG captures on static and high-entropy screens; first and repeat library snapshots; first and repeat V1/V2 listings of the preflight-selected largest allowlisted directory | operation phase evidence, APC and archive, normalized summary |

The launcher is restarted independently for arms that compare observer cost.
Every arm restores the normal launcher whether it succeeds or fails. Benchmark
preflight and cleanup retain the boot identity, installed manifest, settings,
display mode, and launcher state, remove temporary state, and assert that every
boot-loop arming path remains absent.

The Arcade profiling arms never apply a display transaction. The active route is
queried before each arm and verified unchanged afterward. Only the unprofiled
control owns cadence gates; the profiler arms exist to rank list overlay, blend,
copy, and snapshot hotspots for a subsequent optimization experiment.

## Artifact ownership and identity

The typed benchmark workflow owns all temporary runtime environment, tracefs
mounts, gatord processes and PID files, volatile benchmark requests, output
directories, and cleanup assertions. The device launcher and device agent own
their in-process traces. The host owns extraction, schema validation,
correlation, packaging, and the final report.

Every system-wide capture carries a shared manifest with:

- device boot ID;
- installed platform-manifest hash;
- installed GUI and device-agent hashes, agent byte size, and the exact MagiK
  build revision that produced the agent;
- gatord version and executable hash;
- device monotonic clock domain and capture start and end timestamps;
- fixed capture mode and bounded duration;
- local clean revision and installed build revision where available.

A capture is invalid when boot or installed identity changes, the capture is
incomplete, symbols cannot resolve, or cleanup ownership cannot be proven. The
launcher-response Streamline v1 contract remains compatible while adopting the
same manifest.

Generated evidence remains ignored below `build/agent-benchmarks/`. The final
baseline commit curates only a compact summary and exact provenance under
`history/`.

## Counter interpretation

The Cortex-A9 PMU records cycles, instructions, L1 data accesses and refills,
branches, and branch mispredictions for the thread and span that owns the
counter group. Reports derive cycles per frame, instructions per cycle, L1D
refill ratio, and branch-mispredict ratio. Thread-scoped counters do not account
for time when the thread is descheduled, child processes, other workers, or
system-wide contention.

Streamline attributes scheduling, kernel activity, page faults, storage,
network transport, child processes, and cross-thread interference. Its sampled
CPU totals are not substituted for exact per-span PMU deltas. Device-monotonic
phase windows correlate runtime evidence with the system-wide capture without
assuming that host wall time shares the device clock.

PMU and Streamline runs are attribution-only. They cannot qualify animation
cadence or product latency because the observers perturb the workload.
Unprofiled control arms are performance authority. Protocol-v5 presentation
counters are physical cadence authority. Physical repeated vblanks, latch
drops, sequence gaps, and ownership losses are reported independently.

## GUI attribution contract

The benchmark-only GUI controller is dormant during ordinary execution. A
fixed route owns warmup, phase start and end, measurement completion, timeout,
interrupted-input failure, and missing-presentation failure. PMU records drain
after the active window and are written asynchronously outside frame timing.

Measured frame phases are:

- timer dispatch, light bridge synchronization, full bridge synchronization,
  ordinary Slint raster, and forced-full raster;
- Arcade row update, preview cut/fade composition, navigation-transition
  raster, and orientation-transition raster;
- custom-layer generation separately from copying the layer into a hidden
  slot;
- catch-up restoration, base damage copy, preview overlay copy, Arcade overlay
  copy, store publication, post request, and completion polling.

Each measured frame records its logical change class, Slint damage rectangles,
invalid and catch-up bytes, copied rectangles, full-copy state, target slot,
existing wall and thread-CPU telemetry, cache state, rows, pixels, and
protocol-v5 presentation evidence. Completion polling remains separate from
memory-copy work.

`mister-magik-gui-frame-attribution-v1` summarizes cycles per frame, IPC,
refill and mispredict ratios, phase wall time, damage and catch-up
amplification, observer deltas, and correlation between phase outliers and
physical repeated vblanks.

## System, transition, and launch attribution

System-entry capture retains separate C64 and SNES monotonic windows and
correlates scheduler and page-fault activity with descriptor lookup, NavPack
open, row projection, catalog replacement, preview preparation, and CPU1
adoption.

Transition capture retains landscape and portrait evidence and reports each
fade and zoom window separately. Snapshot, raster, portrait composition,
hidden-copy, and confirmation costs remain distinguishable. Existing
qualification scenarios and their thresholds are unchanged.

Launch/return PMU spans distinguish synchronous UI return-capsule construction,
encoding, and save from worker launch preparation, archive extraction, FIFO
request, and acknowledgement handling. Worker profiles feed the existing
process collector. Input-to-loading-frame, loading-to-capsule, preparation,
Main acknowledgement, core-active, context restoration, terminal preview, and
first-correct-present timestamps remain intact.

`mister-magik-launch-return-attribution-v1` reports artifact validity
independently from the five-second product boundary.

## Device-agent attribution

The optimized device-agent build retains resolvable function symbols while
stripping line-level debug information. Reports include installed agent hash
and build revision. This changes neither protocol nor steady-state behavior.

Telemetry phase evidence covers process discovery, `/proc` parsing, CPU,
network and disk reads, status parsing, lease publication, FPGA telemetry,
JSON assembly, and socket write. It records child-process count, files read,
serialized bytes, and sample-deadline overrun without adding measurement-only
filesystem reads or subprocesses.

I/O phase evidence covers raw framebuffer read, content scan, LZ4, RGB
conversion, zlib, CRC, hex encoding, PNG assembly, library snapshot read and
hash, directory enumeration and sort, serialization, and socket write. Each
operation records device-monotonic request, start, and end timestamps, bytes or
entries processed, and peak buffer ownership/RSS while raw and compressed
payloads coexist. Existing authentication, allowlisting, path, request-size,
and allocation bounds remain unchanged.

`mister-magik-agent-observer-attribution-v1` separates GUI frame impact from
agent CPU and wall cost. `mister-magik-agent-io-attribution-v1` normalizes I/O
results by bytes and directory entries.

## Validity and decision gates

An artifact is valid only when all fixed phases complete, every required PMU
span is present with no counter failures or dropped records, every Streamline
capture is complete and identity-bound with resolvable application symbols,
and all restoration and cleanup checks pass. Slow performance alone does not
invalidate evidence.

Authoritative animation windows require zero physical dropped frames, latch
drops, sequence gaps, and ownership losses. Instrumented latency is diagnostic;
only unprofiled controls determine product quality.

An optimization candidate advances when it accounts for at least 10% of the
workload's cycles, at least 1 ms of an interactive frame, or a measurable
increase in repeated vblanks. A non-frame operation must expose at least 5% of
its target phase and project at least 2% end-to-end improvement. Observer
overhead is material at a 2% frame-time regression, a 0.5 ms P99 increase, or
any new physical repeated frame.

The final shortlist ranks whole-operation opportunity, confidence, user
impact, and implementation risk. It contains recommendations only; it does not
include performance optimizations.
