# Production performance review: Cortex-A9, RGB565, and exFAT

Date: 2026-08-21

Frozen source and installed MagiK revision:
`cf799d28c32dbf58929dee82789698891c5310a4`

Installed platform: v0.29. Installed Main revision:
`f290719`

Scope: production launcher, catalog, input, rendering, storage, launch/return,
search, screenshots, and device-agent paths. Experimental effects and particle
workloads were deliberately excluded.

## Executive outcome

The project is not generally frame-budget limited. The production Arcade path
sustained every qualifying 40-second display-route control with zero physically
repeated refreshes, zero ownership loss, and zero latch drops. The important
performance risks are narrower and more actionable:

1. The real input reader and UI both run on CPU1, but the UI is `SCHED_RR` while
   the input reader is ordinary `SCHED_OTHER`. Scheduler tracing measured input
   processing p95 at 27.683 ms and a 45.297 ms maximum. This is a production
   scheduling inversion, not an input-device throughput problem.
2. The catalog namespace walker collects an entire target before the consumer
   can proceed. A full-card run measured 21.527 s of consumer wait, a maximum
   handoff batch of 18,915 entries, 10,271 directory opens, and 83,404 entries.
   Streaming bounded chunks is the highest-confidence catalog optimization.
3. Catalog publication is materially constrained by exFAT durability and copy
   work. Full-card publication took 10.346 s, of which copy/hash took 9.314 s
   for 73.52 MB; the targeted function graph attributed 6.928 s to
   `file_write_and_wait_range` in one fresh durability capture.
4. There are lifecycle defects around response completion and transitions.
   Launcher response confirmed all 17 interactions at the first eligible
   vblank, but only 11 feedback items retired before timeout. Input integrity
   also timed out after matching 46 presses and 46 releases with no held input.
   These are terminal-receipt or feedback-retirement failures, not evidence of
   lost input.
5. Portrait Home-to-Settings reproduced two physical dropped frames at only
   4.509 ms of reported work. A profiled follow-up reproduced isolated portrait
   drops. This points to transition/presentation state, including the identified
   full-raster cache behavior, rather than raw raster capacity.
6. Source review found a static `ModalOverArcade` state that forces a full Slint
   present on every tick. It can permanently prevent the event-driven idle path.
   This has no steady-state regression test and should be fixed before pursuing
   broad renderer micro-optimization.

The best near-term program is therefore: fix scheduling and completion
contracts, stream catalog namespace handoff, make full raster invalidation
preserve Slint's partial cache, and stop steady modal full presents. Do not
replace the proven RGB565/custom-layer architecture or broadly parallelize work
across both cores.

## Method and evidence authority

Phase one used three independent reviewers over the current production source:

- runtime scheduling, input, presentation, and per-core work;
- catalog, search, preview, launch/return, and exFAT data paths;
- Slint composition, dirty regions, custom layers, and RGB565 rendering.

Phase two used only the typed `scripts/agent benchmark` and read-only typed
device interfaces on the real MiSTer. No binary, platform file, catalog format,
or production configuration was changed. The runner reconciled the installed
runtime with the clean source revision. Profiler arms are used only for
attribution; unprofiled controls and FPGA-owned vblank telemetry own cadence
claims.

The device was checked after the campaign. Fault arming was clear, Main was
`LauncherActive`, the launcher was healthy on Home, and the framebuffer was
1280x720 RGB565.

## Hardware model that should govern optimizations

The target is a dual-core Cortex-A9 with a shared memory system and an exFAT SD
card, not a desktop CPU with abundant independent cores and cheap random I/O.
The useful design rules are:

- Keep the latency-critical UI/present path bounded and predictable on CPU1.
- Give CPU0 background work, but do not create permanent worker fan-out. Use
  phase-aware widening only when CPU1 is genuinely idle and every job has a
  short cooperative checkpoint.
- Minimize write amplification, metadata churn, `fsync`, and read-after-write
  hashing on exFAT. Prefer contiguous sequential publication.
- Bound queues by bytes as well as item count. The full build already reached
  113,336 KiB RSS in storage attribution and 121,072 KiB in the PMU rebuild.
- Preserve RGB565 and physical custom layers. Their current production cadence
  is proven; converting everything into Slint would increase blending and copy
  work.
- Treat write-combined framebuffer reads as exceptional. Keep cacheable shadows
  for consumers that need completed pixels.
- Measure physical repeats separately from rejected/superseded latch posts.

## Phase one: production source review

### A. Input and CPU scheduling

#### P0 — CPU1 real-time inversion

`crates/catalog/src/runtime_thread.rs:79-90` assigns the UI a CPU1
`SCHED_RR` policy at priority 10. `apps/mister/src/input_hub.rs:412-423`
places the input reader on CPU1 but leaves it `SCHED_OTHER` at nice -15.

The intended separation therefore fails under sustained UI work: a runnable
real-time UI thread can prevent the thread that produces its input from running.
The scheduler trace confirms this mechanism with tens-of-milliseconds input
reader delays while aggregate CPU1 utilization was only 44.14%.

Recommended change:

- make the input reader's scheduling relationship explicit and testable;
- keep it on CPU1, but ensure it can preempt or promptly run alongside the UI;
- bound every reader critical section and retain a safe ordinary-policy
  fallback;
- qualify with the real proxy-v3 input-latency lab before retention.

Do not move the reader blindly to CPU0. USB interrupt work and background
catalog activity already contend there, and the scheduler trace measured about
575 ms of named USB handler time across the 7.37 s window.

#### P0 — input is observed early but consumed late

The launcher does catalog/control maintenance before draining input
(`apps/mister/src/ui_runner/launcher_loop.rs:5922-5974`, `6349-6359`,
`6530-6559`, then `7205-7214`). An urgent flag should create a bounded fast
lane to drain input before nonessential work. It must not skip lifecycle or
ownership obligations.

#### P1 — avoid allocation and cloning under the input mutex

`apps/mister/src/input_hub.rs:262-297` creates multiple vectors and clones
strings while draining under a mutex. Reuse caller-owned buffers and move
string materialization outside the locked section. This is subordinate to the
scheduling fix: allocator cleanup cannot compensate for starvation.

### B. Slint, composition, and presentation

#### P0 — steady Arcade modal permanently forces presentation

`UiCompositionController::tick` reports a full frame whenever the requested
state is `ModalOverArcade`, even when the state is unchanged
(`apps/mister/src/launcher_runtime/composition.rs:302-315`). That becomes a
permanent wake reason (`launcher_loop.rs:9166-9179`), prevents the idle path
(`9199-9278`), and expands cached damage to the full logical frame
(`10697-10708`).

The documented contract requires a carrier frame while direct-layer retirement
is established, not perpetual full presentation. Track the transition and
retirement receipt explicitly, then allow a static modal to idle. Add a
steady-state `ModalOverArcade` test; current tests cover entry but not the
settled state.

Required device qualification:

- static confirmation dialog over Arcade for at least 40 seconds;
- physical repeats, CPU utilization, Slint raster time, copy bytes, and wake
  reasons;
- direct Arcade layer retirement and restoration correctness.

#### P0 — forced full raster destroys the next partial-render cache

`apps/mister/src/visual_platform.rs:43-55` implements a forced full raster by
switching Slint 1.17.1 from `ReusedBuffer` to `NewBuffer`, then back. The pinned
software renderer clears its partial-render cache when the repaint-buffer type
changes. `NewBuffer` does not rebuild that cache, so the next normal
`ReusedBuffer` pass sees the tree as new and can pay another full raster.

Expose a version-pinned full-dirty seam that keeps `ReusedBuffer`, marks the
full logical region dirty, and refreshes the partial-render dependency cache.
Validate deleted/moved items and rotation coordinates. Compare the forced
destination frame and the immediately following frame; a one-frame benchmark
would miss this cost.

#### P1 — preserve multi-rectangle damage for custom layers

Slint damage remains a fixed-capacity rectangle list for scanout, but
`launcher_loop.rs:9805-9817`, `9916-9952`, and `10014-10020` collapse it to one
bounding box before deciding whether Arcade and preview layers are invalid.
Two disjoint chrome changes can therefore spuriously cross a dense custom layer
and force regeneration.

Pass the logical `DirtyRectList` and test `any(intersection)` in
`ui_frame_target.rs` and `launcher_compositor.rs`. Retain the bounding box only
for legacy telemetry. Test two and three disjoint regions around both the list
and preview.

#### P1 — media progress replaces whole Slint models

`launcher_worker_intents.rs:360-426` rebuilds rows and `SharedString` values for
each media-progress update, `launcher_bridge.rs:542-549` replaces the model,
and the intent dirties the full bridge. Retain a `VecModel`, update changed rows,
and coalesce progress to 50-100 ms unless completion or error requires immediate
visibility.

#### P1 — selection changes scan all cached rows

`launcher_presentation.rs:526-555` walks every cached menu row for a selection
change. Update only the previous and current indices, with stable identity checks
for reorder/filter events.

#### P2 — cache light bridge projections by revision

String projections for light bridge state are rebuilt in the main loop
(`launcher_loop.rs:8498-8522`, `7304-7310`). Retain revision-keyed projections
and update only changed properties.

#### Keep — current custom RGB565 path

The PMU attribution identifies overlay copy, custom layer drawing, Slint raster,
Arcade row generation, and CRT list update as large per-frame consumers. Yet the
unprofiled route matrix has healthy margins and no repeated refreshes. These are
optimized production responsibilities, not automatically defects. Preserve the
custom-layer split and optimize only when a specific interaction fails.

### C. Catalog and exFAT

#### P0 — stream namespace handoff instead of collecting each target

`crates/catalog/src/namespace_walk.rs:132` and `:502` collect a target into a
vector before invoking the visitor, capped at 65,536 entries/16 MiB. The normal
consumer cannot overlap that traversal. A 128 KiB `getdents` buffer is also
allocated for recursive directories (`:588`), and a fallback can re-walk a
target (`:166`).

Replace target-sized collection with bounded chunks, preserving deterministic
ordering and rollback by recording target/chunk offsets in builder recovery
state. Reuse traversal buffers. The measured 21.527 s consumer wait and 18,915
entry batch make this the strongest source-plus-device optimization.

Correctness gates must compare canonical per-game identity, lossless paths,
ordering, published artifact hashes, interruption recovery, and unchanged
rebuild output—not only game counts.

#### P1 — remove the extra prepared-collection walk

Prepared DOS/AO486 collection indexing runs before the normal scan
(`prepared_collections.rs:50,117`; `library_indexer.rs:967`; normal walk at
`catalog_scan.rs:653`). Fuse it with the main traversal or restrict it to roots
that reference prepared payloads. On this card prepared-payload work was only
2.056 s, so it is not the first target.

#### P1 — emit target fingerprints from the walker

Warm resume verification currently sends every file through the consumer to
recreate a fingerprint (`library_indexer.rs:582,700,992`). Have the walker emit
the target fingerprint while traversing so a verified unchanged target can
avoid repeated row materialization.

#### P1 — reduce exFAT publication amplification

`scanner_cache.rs` regenerates the cache directly on exFAT. Shard publication
also copies and hashes completed data. The full-card run measured 108.44 MB of
writes, 1,307 write I/Os, and 15.26 ms average write wait; copy/hash consumed
9.314 s for 73.52 MB.

Experiments should remain transactionally safe and manifest-last:

- write each final shard once to a sibling temporary file;
- compute its hash during that sequential write, not by rereading it;
- `fsync` the file, rename it, then publish the manifest last;
- build the scanner cache in tmpfs only if a strict memory budget is reserved,
  then perform one sequential final copy.

Do not make preallocation the default without a new A/B result. Prior review
rejected blanket preallocation and `copy_file_range` on this exFAT stack.

#### P1 — stream Arcade bootstrap into a bounded queue

Arcade bootstrap buffers its full stream and then fans out across four CPU0 MRA
workers (`library_indexer.rs:852,879,1020`). Start with two workers and a bounded
queue of prefix-read jobs; permit temporary dual-core widening only in the
existing `DualCoreBurst` state with sub-millisecond checkpoints. Prepared work
is currently small, so retain only if whole-card wall time improves without
input/cadence or memory regression.

#### P2 — keep search connections and statements warm

Each search query opens every shard and prepares SQL anew
(`persisted_search.rs:63,133,200,672`). PMU attributed about 691.24 million of
695 million search cycles to SQLite. Current device queries were still usable:
6.339 ms median overall and 25.996 ms p95, with the slow “2 player” query at
25.715 ms median. A long-lived CPU0 search worker with immutable read-only
connections and prepared statements is worthwhile after P0 work, with bounded
connection memory and catalog-generation invalidation.

#### P2 — preview deduplication and negative cache

Selected and prefetched previews can decode the same asset concurrently
(`preview_worker.rs:574,920`), and missing sidecars are repeatedly probed. Add
in-flight key deduplication and a catalog-generation-scoped negative cache.

#### P2 — simplify redundant persisted navigation data

Navigation is stored as a SQLite blob, an adjacent navigation artifact, and a
NavPack. Runtime entry maps NavPack. Remove the unused representation only in a
deliberate schema migration with compatibility evidence. This is capacity and
write reduction, not a current latency priority.

#### P2 — stream whole-file identity work

ROM hashing paths read or copy whole payloads and use bit-oriented CRC code in
places (`software_identity.rs:612,669,785,844`). Use bounded buffered reads and
table/accelerated CRC only after identity parity tests.

#### Keep — NavPack lazy system entry

Ten fresh-process repetitions across six systems produced 60 successful samples:
C64 p95 42 ms, SNES 70 ms, PC88 74 ms, NES 72 ms, BBC 74 ms, and Arcade 85 ms.
NavPack itself consumed only low single-digit millions of cycles in targeted
PMU capture. Do not expand eager resident catalog state to improve this path.

### D. Other runtime paths

#### P1 — cacheable committed-frame shadow for consumers

Analytics streaming snapshots pixels from write-combined scanout on the UI
thread (`launcher_present/orchestrator.rs:920-925,1069-1073`; runtime
`framebuffer/stream.rs:355-405`). Readiness also hashes a full hidden WC frame.
Use the existing cacheable completed-frame representation, or maintain a
cacheable committed shadow, for streaming and readiness. Avoid adding an
unconditional extra full-frame copy: observer attribution did not demonstrate
a cadence failure in the current configuration.

#### P1 — move status/event serialization off the hot loop

The one-second runtime status path constructs strings and summaries before a
`try_lock`, and JSON event logging synchronously writes `/tmp` from the UI
thread. Build only after admission, publish immutable snapshots to a low-priority
CPU0 worker, and keep a bounded overwrite queue.

#### P2 — agent capture defaults

Agent I/O measured static capture at roughly 36-38 ms raw, 37-43 ms LZ4, and
106-125 ms PNG; high-entropy PNG reached 159-166 ms. LZ4 nearly halves peak
payload versus raw without PNG CPU cost. Directory protocol V2 took 112-131 ms
versus 165-180 ms for V1 over 987 entries. Prefer LZ4 and V2 where consumers
support them; this is operational tooling, not launcher hot-path work.

## Phase two: real-hardware results

### Benchmark disposition

| Scenario | Result | Principal evidence |
| --- | --- | --- |
| `launcher-response` | Failed terminal gate | 17/17 confirmed on first eligible vblank; 11/17 feedback hidden; 6 outstanding |
| `gui-frame-attribution` | Passed | zero repeats/drops/gaps/loss; 1.628M cycles/frame, IPC 0.368 |
| `scheduler-trace` | Passed | CPU0 18.63%, CPU1 44.14%, dual overlap 9.50%; input p95 27.683 ms, max 45.297 ms |
| `input-latency-lab` | Blocked | requires Main proxy v3; installed platform exposes proxy v2 |
| `agent-observer-attribution` | Passed | no drops; no stable observer regression established |
| `storage-attribution` | Passed | full-card 143.05 s; 139.31 MB read, 108.44 MB written |
| `catalog-attribution-control` | Failed instrumentation | bounded device telemetry emitted no samples; not retried |
| `catalog-full-build-rebuild` | Passed | first 140.581 s; warm 155.347 s; rebuild 61.523 s |
| `pmu-profile` | Passed | fresh 101.687 s, rebuild 82.783 s, rebuild-all 115.940 s |
| `search` | Timing passed, UI gate failed | 6.339 ms median, 25.996 ms p95; cache/UI verification unavailable |
| `system-entry` | Passed | all 69 systems ready; no unready rows |
| `launch-return-attribution` | Failed evidence gate | no complete identity-bound result; no performance conclusion |
| `screensaver` | Blocked | no existing usable cached catalog for this scenario |
| `navigation-transitions` | Failed | screensaver profile did not complete |
| `settings-navigation` | Retained failure | landscape clean; portrait first leg 2 physical drops |
| `settings-navigation-pprof` | Attribution complete | reproduced isolated portrait drops; profiler startup leg not cadence authority |
| Arcade velocity attribution | Passed | 60 Hz landscape control, zero repeats; foreground p99 5.228 ms |
| Six Arcade turbo route controls | Passed | every route zero repeats, loss, gaps, and latch drops |
| `agent-io-attribution` | Passed | LZ4 and directory V2 are best operational defaults |
| `cold-boot` | Passed | Linux boot to first present 12.420 s |
| `cold-boot-pprof` | Passed attribution | Linux to first 13.620 s with profiler overhead |
| `launcher-response-attribution` | Failed terminal gate | 30/33 confirmed, 28/32 hidden, one outstanding |
| `input-integrity` | Failed terminal gate | 46 presses = 46 releases, final held state false |
| `system-entry-critical-profile` | Passed attribution | NavPack cost small |
| `system-entry-critical-confirm` | Passed | 60/60 fresh-process samples successful |
| `catalog-lifecycle` | Partial/failed cadence gate | first visible 12.543 s, ready 140.804 s; intro cadence unavailable |
| `catalog-attribution-function-graph` | Passed attribution | identical fingerprints; namespace and durability captures complete |

Blocked and failed instrumentation is reported rather than silently retried.
Mutation-like or reboot-capable actions were not replayed. The only cold-boot
workflow was supervised and bounded.

### Frame and PMU attribution

The 720p60 GUI attribution control completed with zero physical repeats, latch
drops, sequence gaps, or ownership loss. Across 116 sampled frames it measured
188.8 million cycles, or 1.628 million cycles/frame, with IPC 0.368 and 7.51%
L1D refill rate.

Largest nested PMU spans were:

| Span | Cycles | Approx. cycles/frame |
| --- | ---: | ---: |
| Overlay copy | 59.68 M | 514 k |
| Slint raster | 31.56 M | 272 k |
| Custom layer | 23.99 M | 207 k |
| Arcade row generation | 18.82 M | 162 k |
| CRT list update | 16.94 M | 146 k |
| Post-processing | 18.40 M | 159 k |
| Completion polling | 13.10 M | 113 k |

These spans overlap and are not additive wall time. The overlay copy span had
IPC 0.296 and 11.88% L1D refill, making it memory-sensitive, but the route
controls show sufficient production margin.

### Display route qualification

All six production turbo controls ran for 40 seconds and passed authoritative
physical cadence:

| Route | Refresh | Foreground p99 | Foreground max | Physical drops |
| --- | ---: | ---: | ---: | ---: |
| HDMI 720 landscape | 60.001 Hz | 7.617 ms | 8.835 ms | 0 |
| HDMI 720 portrait-left | 60.001 Hz | 12.316 ms | 13.652 ms | 0 |
| HDMI 1080 landscape | 60.001 Hz | 6.538 ms | 7.652 ms | 0 |
| HDMI 1080 portrait-left | 60.001 Hz | 9.485 ms | 10.771 ms | 0 |
| CRT 240 portrait-left | 60.053 Hz | 9.466 ms | 14.379 ms | 0 |
| CRT 288 portrait-left | 50.429 Hz | 10.114 ms | 11.150 ms | 0 |

Each also recorded zero ownership loss, sequence gaps, and latch drops. This is
strong evidence against broad renderer rewrites. The isolated portrait Settings
failure should be treated as a transition contract bug.

### Catalog attribution

Full-card storage attribution covered 40,059 games across 69 systems:

- total elapsed: 143.05 s;
- scan: 70.190 s;
- prepared payload: 2.056 s;
- pipeline: 66.525 s;
- captured namespace: 83,404 entries, 72,990 files, 10,271 opens;
- consumer wait: 21.527 s;
- consumer active: 44.905 s;
- maximum handoff batch: 18,915 entries;
- projection: 43.383 s;
- reconciliation: 36.841 s;
- shard build: 28.881 s;
- publication: 10.346 s;
- copy/hash: 9.314 s for 73.52 MB;
- scanner cache: 5.429 s;
- high-water RSS: 113,336 KiB.

The full-build sequence was not improved by a nominal warm run: first clean
140.581 s, warm clean 155.347 s, unchanged rebuild 61.523 s. This makes warm
verification and storage state explicit optimization targets; it is not valid
to assume page cache alone makes the build faster.

PMU spans for a fresh profile attributed 97.6 billion cycles at IPC 0.601.
Largest nested spans were persist 27.303 B, execution/walk 16.567 B, scan
14.712 B, prepare 12.352 B, search index 6.909 B, validate 3.853 B, search rows
3.400 B, games 2.893 B, bootstrap 2.234 B, navigation 2.184 B, and copy/hash
0.951 B cycles.

The targeted function-graph run used only real Arcade, SNES, and C64 content,
kept identical catalog fingerprints, and captured namespace and durability
groups. Fresh timings were 90.969 s and 87.754 s respectively; rebuilds were
30.407 s and 29.878 s. In the durability fresh capture,
`file_write_and_wait_range` accumulated 6.928 s. Function-graph timings are
attribution only.

### Startup and system entry

Authoritative cold boot to first present was 12.420 s:

- Linux agent start: 6.430 s;
- initial Main: 7.524 s;
- launcher exec to MagiK process: 0.536 s;
- MagiK process to first present: 2.133 s;
- process to startup clock: 0.824 s;
- startup clock to first present: 1.309 s.

Host recovery time is not product startup time. The profiled boot was slower and
mostly unsymbolized, so it does not justify a code change.

System entry is already bounded. The broad scenario entered all 69 systems
without an unready row, and the fixed critical confirmation put even Arcade at
85 ms p95. Preserve lazy NavPack mapping.

## Ranked implementation program

### Tier 0: correctness and benchmark authority

1. Correct the UI/input scheduling relationship and update the installed Main
   proxy so `input-latency-lab` can run.
2. Fix feedback-hidden and terminal-receipt accounting. Add a bounded drain at
   scenario teardown so matched physical input cannot fail only because the
   final receipt was never published.
3. Add a dedicated static Arcade-modal benchmark and a two-frame forced-raster
   benchmark.
4. Make the Settings transition gate preserve per-leg physical telemetry and
   distinguish setup/carrier frames from steady work.
5. Restore an authoritative startup-intro cadence window before claiming catalog
   work is invisible to the UI.

### Tier 1: highest-confidence production changes

1. Stream namespace entries in deterministic bounded chunks; reuse traversal
   buffers and preserve interruption recovery.
2. Scope `ModalOverArcade` full presentation to transition/retirement only.
3. Preserve Slint `ReusedBuffer` cache across explicit full invalidation.
4. Preserve multi-rectangle damage through custom-layer intersection tests.
5. Add urgent input drain and reuse its buffers after scheduling is fixed.
6. Hash shards during their single sequential final write; reduce exFAT rereads
   without weakening manifest-last recovery.
7. Retain and diff media-progress/selection models.

### Tier 2: measured but lower-priority work

1. Persistent read-only search connections and prepared statements.
2. Target-level fingerprint emission for warm verification.
3. Bounded two-worker Arcade metadata queue with temporary, checkpointed
   dual-core burst.
4. Cacheable committed-frame access for streaming/readiness.
5. Async runtime-status and event serialization.
6. Preview in-flight deduplication and negative sidecar caching.
7. Scanner-cache tmpfs staging only under an explicit RSS ceiling.
8. Stream raw media packs and ROM identity hashing.
9. Remove redundant navigation persistence in a future schema migration.

## Changes not recommended

- Do not replace Main with the Slint binary or bypass Main's video ownership.
- Do not move all background work onto CPU1; phase-aware widening is the only
  safe use of the second core.
- Do not add permanent four-way worker pools on a two-core system.
- Do not expand eager resident catalog state to improve already-fast system
  entry.
- Do not replace custom Arcade/preview layers with full-screen Slint content.
- Do not add unconditional full-frame shadows or copies without observer A/B
  evidence.
- Do not treat latch-post rejection as a dropped frame.
- Do not reintroduce previously rejected JSON replay, global autocomplete,
  blanket preallocation, `copy_file_range`, parallel runtime classification, or
  on-media staging without new isolated evidence.
- Do not optimize experimental effects; none were in scope or executed.

## Acceptance gates for retained optimizations

Every retained production optimization should pass:

1. Exact source/runtime identity and clean device reconciliation.
2. Canonical per-game IDs, lossless paths, ordering, search corpus/rank, launch
   contracts, and published artifact hashes where catalog data changes.
3. Interrupted-build recovery and manifest-last publication validation.
4. Peak RSS no worse than the current confirmed control, unless a separately
   approved capacity budget exists.
5. Input integrity and latency with real proxy-v3 transport.
6. Zero physical repeated refreshes, ownership loss, sequence gaps, and latch
   drops on the affected display routes.
7. Unprofiled before/after controls; PMU, pprof, function graph, and Streamline
   may explain but never qualify cadence.
8. A rollbackable, production-format-compatible delivery path.

## Evidence locations

Principal retained artifacts are under `build/agent-benchmarks/`:

- `gui-frame-attribution/1787280895/summary.json`
- `scheduler-trace/1787280970/summary.json`
- `agent-observer-attribution/1787281060/summary.json`
- `storage-attribution/1787281206/summary.json`
- `catalog-full-build-rebuild/1787281485/summary.json`
- `pmu-profile/1787281874/summary.json`
- `search/1787282254/timing/summary.json`
- `system-entry/1787282282/summary.json`
- `settings-navigation/1787282711/report.md`
- `settings-navigation-pprof/1787282771/summary.json`
- `arcade-velocity-scroll-attribution/1787282839/summary.json`
- `arcade-velocity-scroll-attribution/1787283136/summary.json`
- `arcade-velocity-scroll-attribution/1787283222/summary.json`
- `arcade-velocity-scroll-attribution/1787283311/summary.json`
- `arcade-velocity-scroll-attribution/1787283397/summary.json`
- `arcade-velocity-scroll-attribution/1787283473/summary.json`
- `arcade-velocity-scroll-attribution/1787283557/summary.json`
- `agent-io-attribution/1787283638/summary.json`
- `cold-boot/1787283684/summary.json`
- `cold-boot-pprof/1787283744/summary.json`
- `system-entry-critical-profile/1787283905/summary.json`
- `system-entry-critical-confirm/1787283928/summary.json`
- `catalog-lifecycle/1787284162/catalog-lifecycle.log`
- `catalog-attribution-function-graph/1787284421/summary.json`

Benchmark artifacts are local evidence and are normally ignored by Git. This
review is the durable synthesis; production source was not modified.
