# Production GUI and catalog performance review — 2026-08-23

## Scope and evidence boundary

This review covers the production MiSTer MagiK GUI, launcher, catalog, search,
preview/media, storage, and framebuffer paths on the dual-core Cortex-A9 and
the exFAT-backed MiSTer SD card.

The review intentionally excludes:

- experimental visual effects and transition-effect comparisons;
- particle experiments and framebuffer demonstration applications;
- FPGA changes and unqualified platform substitutions;
- desktop-only rendering behavior except where it consumes production evidence.

Work runs on branch `nigel/production-performance-review-2026-08-23`, based on
the clean local `main` tip that was seven commits ahead of `origin/main` when
the branch was created.

The investigation has two deliberately separate evidence phases:

1. **Phase one — code review.** Findings in this section were frozen before any
   fresh device benchmark was run. Four independent read-only agents reviewed
   GUI/rendering, catalog/storage, concurrency/core placement, and benchmark
   coverage. Their claims were reconciled against current production source.
2. **Phase two — exact-device evidence.** The later section records fresh typed
   `scripts/agent benchmark` results. Unprofiled controls and protocol-v5
   physical presentation evidence remain performance authority; PMU, pprof,
   tracefs, and Streamline runs provide attribution only.

No phase-one ranking treats a prior measurement as fresh evidence. Where the
current code suggests an opportunity but repository policy records an earlier
closed gate, the item is explicitly retained as a hypothesis for remeasurement
rather than a recommended rewrite.

## Phase one: production architecture review

### Core use cases reviewed

The code review followed the user-visible paths that dominate ordinary device
experience:

1. launcher startup and first correct presentation;
2. Home focus movement and confirmed feedback;
3. system entry, initial NavPack mapping, first rows, and first preview;
4. Arcade held scrolling, row raster, direct-layer composition, preview
   selection, prefetch, and hidden-slot presentation;
5. resident and persisted search while typing;
6. Settings and modal entry/retirement;
7. catalog first build, first-visible publication, incremental refresh,
   full rebuild, interrupted-build resume, and generation adoption;
8. current and newly downloaded media-pack validation/publication;
9. ROM identity hashing and preview-pack reads;
10. game launch, return, context restoration, and ordinary launcher recovery.

### Current dual-core design is directionally correct

The existing production topology already reserves the latency-sensitive core
and applies bounded background parallelism:

- launcher UI runs on CPU1 with nice -10 and real-time round-robin priority 10;
- input reading runs on CPU1 at nice -15;
- normal catalog building and walking run on CPU0 at reduced priority;
- system-entry preparation, selected preview loading, and preview composition
  run on CPU0;
- catalog work may use both cores only before first-visible publication or
  after idle settling, returns to CPU0 during animation, and parks during
  interaction.

Source: [`runtime_thread.rs`](../crates/catalog/src/runtime_thread.rs),
[`ui_runner.rs`](../apps/mister/src/ui_runner.rs), and
[`launcher_loop.rs`](../apps/mister/src/ui_runner/launcher_loop.rs).

This means a general-purpose thread pool, sustained background work on CPU1,
or broader real-time scheduling is not an optimization target. Those changes
would make latency and cache interference less predictable. The best current
opportunities reduce duplicated storage work, per-item handoffs, allocation,
lock hold time, and unnecessary cache-line ownership transfers while retaining
the established interaction parking policy.

### Ranked phase-one opportunity map

| Rank | Opportunity | Code status | Expected bottleneck | Risk | Primary exact-device gate |
|---|---|---|---|---|---|
| P0 | Skip full shard reconstruction when a media pack and sidecar are already current | Observed avoidable work | exFAT reads, SQLite open/row materialization, NavPack decode/hash | Medium-high | warm-launch storage attribution plus media persistence |
| P0 | Batch walker-to-indexer file handoff | Observed per-file synchronization/allocation; gain is a hypothesis | channel wakeups, allocator, cross-core cache traffic | Medium | PMU catalog spans plus two lifecycle controls |
| P1 | Bound/coalesce catalog progress without dropping terminal events | Observed unbounded progress path | memory growth, string allocation, UI adoption latency | Low-medium | catalog lifecycle/resume with queue telemetry |
| P1 | Remove repeated Arcade row/filter allocations | Observed per-row/per-frame allocation | raster CPU, allocator contention, L1/L2 churn | Medium | Arcade control/PMU plus pixel and repeat gates |
| P1 | Build Arcade ROM inventory once per catalog operation | Observed duplicate directory scans | exFAT metadata, BTree allocation, hashing | Low-medium | corpus inventory plus catalog build/rebuild |
| P1 | Move preview metadata I/O outside the shared cache mutex | Observed lock across filesystem metadata calls; user impact is a hypothesis | priority inversion, exFAT tail latency | Medium | preview attribution under concurrent SD activity |
| P1 | Eliminate unchanged catalog work-mode RMWs | Observed AcqRel swap on every launcher iteration | cross-core coherency, cache-line bouncing | Low | scheduler trace, GUI PMU, launcher response |
| P1 | Reuse preview compressed-input scratch | Observed fresh payload allocation | allocator CPU and cache churn | Low-medium | preview attribution and Arcade scroll |
| P2 | Tune durable resume batch/barrier frequency | Observed frequent `sync_data` plus FULL SQLite commit; dominance unproven | exFAT flush latency, write amplification | High | resume validation and storage attribution |
| P2 | Add cancellable, bounded NavPack prelude warming | Existing warm method is not used on normal generation open | cold exFAT open/mmap/page faults | Medium | system-entry profile/confirm/qualification |
| P2 | Prefetch adjacent lazy NavPack pages off CPU1 | UI can trigger lazy 64-row materialization | UI-thread faults and allocation | Medium-high | Arcade control/PMU/Streamline |
| P2 | Make in-flight preview prefetch cancellable | Prefetch checks foreground ownership only before a full read/decode | block-I/O contention and selected-preview tail | Medium-high | preview opportunity gate and system entry |
| P2 | Revisit broad-damage threshold and contiguous NEON copies | Existing 85% promotion and mixed scalar/NEON routes | memory bandwidth, cache pollution | High | NEON attribution plus route cadence/pixel parity |
| P2 | Select streaming namespace walk before a known overflow | Current bounded capture can restart from the beginning | duplicate directory traversal | Medium-high | corpus inventory and lifecycle fallback counters |
| P2 | Add memory headroom to tmpfs shard admission | Current gate uses tmpfs free blocks and a shard estimate only | reclaim/OOM risk versus exFAT fallback | High | largest-shard build with cold/hot preview cache |

### P0: unchanged media causes full catalog-shard reconciliation

When a requested pack is already `Current`, the media worker still inserts it
into `pending_reconciliation` in
[`media.rs`](../apps/mister/src/launcher_runtime/media.rs). Completion then
processes each system independently. Production reconciliation opens the
manifest and binding, fully opens the SQLite and adjacent navigation artifacts,
loads the sidecar index, hashes and decompresses navigation, compares the
embedded copy, reconstructs games, and only then determines whether preview
availability changed.

This is the strongest code-proven warm-path storage opportunity because the
classification result already establishes a stable media identity, yet that
identity is not represented in catalog-generation metadata strongly enough to
permit a no-op. On a large current media set, it can turn a cheap startup check
into one full shard and sidecar pass per system.

Recommended direction:

- bind the media pack and index SHA identities into immutable shard or registry
  metadata;
- when active generation and both identities match, return without opening the
  shard;
- if multiple systems genuinely change, reconcile them into one generation and
  one publication transaction rather than one generation per completion.

The risk is correctness, not implementation complexity: stale preview
availability must remain impossible after a pack replacement or sidecar repair.
Phase two must first quantify warm bytes read, SQLite opens, navigation decode,
sidecar time, allocation, and total per-system reconciliation latency.

### P0: catalog walker hands off one allocated event per file

The catalog walker sends one `DiscoveryEvent::File` per candidate over an
8,192-entry synchronous channel in
[`catalog_scan.rs`](../crates/catalog/src/catalog_scan.rs). The consumer performs
one receive per event and converts each file into a fresh one-element vector in
[`library_indexer.rs`](../crates/catalog/src/library_indexer.rs).

The design correctly overlaps exFAT walking/classification with downstream
indexing, but its unit of synchronization is unnecessarily small for a
dual-core in-order processor. A bounded batch of roughly 32–64 entries is a
plausible improvement because it can reduce wakeups, allocator traffic, and
shared-cache-line transfers while retaining pipeline overlap.

This remains a measured hypothesis. Any batching must preserve target
boundaries, deterministic ordering, fingerprints, cancellation, durable resume
semantics, and cooperative checkpoints. Existing telemetry already exposes
send time/count, slow sends, receive wait, consumer work, and batch size, so the
decision does not require blind implementation.

### P1: progress publication is not backpressured to UI consumption

Catalog worker messages use an unbounded channel in
[`catalog_worker.rs`](../apps/mister/src/ui_runner/catalog_worker.rs), while the
launcher consumes at most two messages per frame in
[`launcher_scheduler.rs`](../apps/mister/src/ui_runner/launcher_scheduler.rs).
Artifact publication can emit numeric progress for every 256 KiB copy chunk in
[`sqlite_catalog.rs`](../crates/catalog/src/sqlite_catalog.rs), and numeric
progress is immediately publishable.

During large copies the producer can outpace a deliberately bounded UI
consumer. The result is avoidable strings and queued stale progress, increased
RSS, and potential delay before the UI observes a meaningful terminal state.

Recommended direction: carry current numeric progress in a one-slot
latest-value mailbox or coalesce by percentage/time, while keeping lifecycle,
failure, and completion events on a small reliable bounded channel. Terminal
events must never be dropped or hidden behind stale progress.

### P1: Arcade row rendering retains avoidable heap churn

The direct Arcade renderer rerasterizes filter rows when an exposed band
intersects them. Normal and filter row paths allocate a temporary `Pixel` row
and then collect a second RGB565 vector in
[`arcade_list_renderer.rs`](../apps/mister/src/arcade_list_renderer.rs). Cached
game-row blits also allocate a `changed_rows` vector.

The likely cost is not only allocation time. Because the UI owns the active
frame path on CPU1, allocator metadata touched by CPU0 workers can create
cross-core contention and cold data on an already tight cadence path.

Recommended direction:

- cache filter rows under the same exact invalidation contract as game rows;
- raster directly into RGB565 storage where possible;
- replace `changed_rows` with fixed-capacity storage or a deterministic second
  loop;
- retain exact CRT fill classification, favorites, selection state, damage-run
  reconstruction, and pixel output.

The authoritative gate is not profiler time. It is identical terminal pixels,
zero protocol-v5 repeated vblanks, and improved or unchanged unprofiled
40-second Arcade controls.

### P1: ROM inventory is reconstructed repeatedly

Resume binding, later Arcade eligibility, and catalog-stamp construction each
have an Arcade ROM inventory path in
[`library_indexer.rs`](../crates/catalog/src/library_indexer.rs) and
[`catalog_stamp.rs`](../crates/catalog/src/catalog_stamp.rs). Inventory creation
enumerates up to four ROM directories, lowercases names into `BTreeSet`s, and
hashes the ordered names in
[`arcade_rom_inventory.rs`](../crates/catalog/src/arcade_rom_inventory.rs).

One immutable build-scoped inventory should be sufficient for all three
consumers. Sharing it removes duplicate `read_dir`, string allocation, tree
insertion, and hashing while making the fingerprint/eligibility contract more
obviously consistent. A sorted vector plus dedup may be more cache-friendly
than node-based insertion, but that representation change should be considered
only after the duplicate scans are removed and exact ordering is protected.

### P1: preview cache metadata checks hold a global lock across exFAT

Archive sidecar metadata expires after five seconds in
[`preview_worker.rs`](../crates/catalog/src/preview_worker.rs). On expiry, the
shared cache mutex remains held while archive and index metadata are fetched
from storage. Selected and prefetch workers share this cache.

The lock scope is code-proven; the resulting priority inversion is a
measurement hypothesis. A low-priority prefetch blocked in metadata lookup may
prevent the foreground selected-preview request from acquiring the cache.

Recommended direction: snapshot cached identity under the lock, release it
before filesystem calls, and reacquire to conditionally publish the result.
Longer term, explicit generation invalidation after atomic media publication is
preferable to frequent TTL probing if it can retain replacement correctness.

### P1: unchanged work-mode requests still transfer cache-line ownership

`set_work_mode` in
[`cooperative_work.rs`](../crates/catalog/src/cooperative_work.rs) performs an
AcqRel atomic swap even when the requested mode has not changed. The launcher
invokes catalog work-mode selection every loop iteration from CPU1 while the
catalog thread repeatedly reads the same state on CPU0.

A load/early-return or caller-side last-mode cache can reserve the RMW and
condition-variable notification for real transitions. This is a low-risk,
likely small gain: epoch changes and paused-to-running wake semantics must remain
exact. Scheduler trace and PMU evidence should reject it if cross-core refill,
frame CPU, and response distributions do not measurably improve.

### P1: preview payload allocation can be reused

Selected and prefetch workers retain an open archive but allocate and zero a new
compressed payload vector for each indexed preview load in
[`preview_worker.rs`](../crates/catalog/src/preview_worker.rs). The decoded RGB565
allocation is retained by the cache and is useful; the compressed-input buffer
is scratch and can be reused per worker.

The safe direction is a capacity-bounded scratch buffer that grows only within
existing entry-size limits and does not retain a pathological pack entry
forever. This should be evaluated together with in-flight prefetch cancellation,
because allocator reduction does not solve SD-queue interference.

### P2: durability barriers may dominate resumable catalog builds

Durable scan batches are capped at 16 targets or 2 MiB in
[`library_indexer.rs`](../crates/catalog/src/library_indexer.rs). Each batch
reopens/seeks the compact frame file, appends compressed records, calls
`sync_data`, and commits a separate SQLite transaction configured with
`journal_mode=DELETE` and `synchronous=FULL` in
[`build_progress.rs`](../crates/catalog/src/build_progress.rs).

On exFAT, the data barrier plus FULL metadata commit may dominate cold build
time and write amplification. It is intentionally ranked P2/high risk because
larger batches increase lost work after power interruption. Phase two must
separate `sync_us`, commit time, bytes written, build wall time, and recovery
behavior before any policy change.

### P2: bounded idle warming can improve system entry

[`lazy_sharded_reader.rs`](../crates/catalog/src/lazy_sharded_reader.rs) already
has a method that maps populated NavPacks and faults bounded entry preludes and
first viewports. Normal generation open in
[`launcher_scheduler.rs`](../apps/mister/src/ui_runner/launcher_scheduler.rs)
retains the reader but does not use that warm path. System activation later pays
descriptor lookup, open, mmap, validation, and first-row work on the CPU0
system-entry worker.

A safe experiment would warm only likely next systems after input readiness and
idle settling, one system at a time, newest-wins, with immediate cancellation on
input and a strict page-cache/RSS budget. Warming every system eagerly is not
recommended.

Relatedly, NavPack rows use lazy 64-row pages and only the first ten rows are
materialized during open. Direction-aware adjacent-page preparation can keep
page faults and row allocation off CPU1, but should be disabled during unstable
turbo direction and bounded by generation tokens and residency limits.

### P2: preview prefetch needs a stronger foreground cancellation point

Selected and prefetch preview loaders are separate CPU0 workers. Prefetch checks
foreground ownership before a complete archive read/decode, but an operation
already in flight can continue to occupy CPU0 or the block queue after a selected
request arrives.

Bounded cancellation checks could improve selected-preview tails, but rapid
scrolling can turn cancellation into wasted SD bandwidth. The existing
`preview-work-attribution` opportunity gate must remain decisive: do not add the
complexity unless selected-preview latency improves without increasing duplicate
work or harming cadence.

### P2: NEON and damage tuning must be span-driven

Damage wider than 85% is promoted to full-row copying in the framebuffer target.
Strided hidden-slot copies already have a production NEON helper processing 32
pixels per loop with prefetch, while contiguous and same-position rectangle
copies use slice or row copies in
[`scanout_slots.rs`](../mister/platform/runtime/src/framebuffer/scanout_slots.rs).

Possible work:

- tune the broad-damage threshold by route from copied versus wasted bytes;
- add a contiguous NEON copy only if PMU shows the eligible spans consume at
  least ten percent of cycles;
- keep the scalar fallback and exact pixel contract.

This is high risk because an apparently faster copy can increase memory
bandwidth, evict useful UI/catalog data, or produce catastrophic visual errors.
No change is justified without `neon-attribution`, route-specific unprofiled
controls, pixel parity, and zero repeated vblanks.

### Lower-priority measured hypotheses

- **Large namespace fallback:** the bounded fd-relative walker restarts with
  `WalkDir` after failure or budget overflow. If real targets exceed 65,536
  entries, 16 MiB retained paths, or 64 open directory FDs, they can pay nearly
  two traversals. Use existing fallback/restart/peak metrics before changing
  target-level atomicity.
- **Tmpfs admission:** shard staging currently uses tmpfs free space and a shard
  size estimate, not `MemAvailable`, process RSS, or decoded-preview occupancy.
  Test the largest shard with empty and full preview caches before adding memory
  pressure to the admission rule.
- **Completion polling:** physical latch confirmation uses repeated status reads
  and `yield_now`. Existing completion poll, CPU, and wall telemetry must show a
  material cost before any bounded backoff experiment; sleeping too long would
  directly harm cadence and input response.
- **Home delegate cardinality:** both HDMI and CRT views instantiate their menu
  delegates behind a clipped viewport. Measure realistic 4/32/128-row models
  before considering visible-window projection because retained row identity and
  selection feedback are correctness-sensitive.
- **Disabled profiling allocation:** ordinary rendering collects Slint damage
  into a `Vec` before the profile receiver returns inactive. Guarding collection
  or passing the existing bounded dirty list is a low-risk micro-optimization,
  but should remain below user-visible storage and row-raster work.
- **Routine coordination logging:** catalog work ownership logs acquire a process
  mutex, allocate a buffer, and synchronously write on acquire/yield/release.
  Measure sink bytes and syscall cost before replacing useful diagnostics with
  periodic counters.

### Explicit non-opportunities and safety boundaries

- Do not increase concurrent exFAT media writes. Production deliberately uses a
  single downloader and serial publication.
- Do not add a general thread pool or move sustained background work to CPU1.
- Do not broaden real-time scheduling beyond the established UI/input contract.
- Do not add preview in-flight deduplication without clearing the existing
  duplicate-work opportunity gate.
- Do not retain persisted-search workers solely because the code opens per
  query. The fallback creates a thread, opens SQLite, prepares statements,
  materializes matches, and globally sorts multi-shard results, but repository
  policy records prior open/preparation cost below its retained-worker gate and
  the ordinary Arcade UI uses resident search. Phase two should remeasure broad
  nonresident prefixes before reopening that design.
- Do not weaken NavPack/shard validation, resume ordering, or atomic publication
  to save I/O without equivalent corruption and interruption evidence.
- Do not infer frame cadence from latch-post drops. Only protocol-v5 repeated
  vblanks identify physical refreshes that reused an old frame.

## Phase two: fresh exact-device campaign

### Execution policy

Phase two uses only the typed `scripts/agent benchmark [SCENARIO]` interface.
Runs are sequential because they share one physical MiSTer and many temporarily
restart the launcher. No benchmark builds, deploys, or replaces platform files.
Catalog mutation uses isolated `/tmp` or Dev-only exFAT roots and must prove the
production registry unchanged. Only typed cold-boot scenarios may issue one
bounded Linux reboot.

Profiled timings are never compared directly with unprofiled product gates.
Attribution runs answer where CPU, scheduling, cache, or I/O time is spent;
unprofiled controls answer whether the product meets latency and cadence goals.

### Production-only matrix

The matrix intentionally omits compatibility aliases, particles, navigation
effect POCs, orientation-transition effects, transition Streamline, and the
input-latency obstruction laboratory.

| Group | Scenarios | Status |
|---|---|---|
| Interaction correctness | `input-integrity`, `modal-input`, `launcher-response` | Pending |
| GUI controls | `settled-composition`, `bridge-model-churn`, `settings-navigation`, `arcade-velocity-scroll`, `screensaver` | Pending |
| System entry and resident data | `catalog-corpus-inventory`, `search`, `system-entry`, `system-entry-critical-confirm`, `system-entry-qualification`, `preview-work-attribution`, `rom-identity-hashing`, `media-pack-persistence` | Pending |
| Catalog controls | `catalog-build-rebuild`, `catalog-resume-validation`, `catalog-full-build-rebuild`, two `catalog-lifecycle` controls | Pending |
| Catalog attribution | `storage-attribution`, `catalog-attribution-control`, `catalog-attribution-pprof`, `catalog-attribution-pmu`, `catalog-attribution-storage`, `catalog-attribution-function-graph`, `catalog-attribution-streamline`, `catalog-attribution-report` | Pending |
| GUI and CPU attribution | `gui-frame-attribution`, `scheduler-trace`, `launcher-response-attribution`, `arcade-velocity-scroll-attribution`, `settings-navigation-pprof`, `agent-observer-attribution`, `agent-io-attribution`, `pmu-profile`, `neon-attribution`, `streamline` | Pending |
| System-entry attribution | `system-entry-critical-profile`, `system-entry-critical-streamline` | Pending |
| Launch/return | `launch-return`, `launch-return-attribution` | Pending |
| Reboot/startup | `cold-boot`, `cold-boot-pprof`, then fresh-catalog variants last | Pending |

`launch-return-once` is excluded from the unattended matrix because it is an
attended USB Video incident route, intentionally leaves the terminal core state
unrepaired, and is not needed to profile ordinary launch/return. Fallback and
compatibility aliases are also excluded because they do not add an independent
production hot path.

### Phase-two results

Pending fresh exact-device execution. Each result will record:

- exact local and installed identity;
- scenario timestamp and artifact directory;
- pass/fail/blocked state and restoration result;
- authoritative versus attribution-only status;
- core latency, cadence, CPU/PMU, scheduler, RSS/HWM, block-I/O, and exFAT
  metrics exposed by that scenario;
- which phase-one hypothesis the evidence confirms, rejects, or leaves open.
