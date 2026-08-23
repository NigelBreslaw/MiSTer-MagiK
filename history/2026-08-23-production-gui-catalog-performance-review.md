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

Phase one ran on branch `nigel/production-performance-review-2026-08-23`, based
on the clean local `main` tip that was seven commits ahead of `origin/main` when
the branch was created. Those additional commits touched only FPGA diagnostic,
device-agent, and supporting evidence paths. Before phase two, an exact diff
confirmed that `apps/mister`, `crates/catalog`, and `mister/platform/runtime`
were byte-identical to the qualified `origin/main` base. Hardware evidence is
therefore collected on
`nigel/production-performance-review-2026-08-23-qualified`, carrying this report
but excluding the unrelated unqualified FPGA diagnostic changes.

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

## Phase two: exact-device campaign

### Execution and authority policy

Phase two used only the typed `scripts/agent benchmark`, `scripts/agent device`,
and `scripts/agent diagnose` interfaces. Runs were serialized because they share
one physical MiSTer. No benchmark built, deployed, or replaced platform files.
The qualified delivery was a no-op: the exact production runtime was already
installed.

The exact identity for all reported controls and profiles was:

| Component | Identity |
|---|---|
| Platform | `platform-v0.29`, bundle `67c943bddf3325f82d6e6666f6046b16dab9d5a972295b0167054b181443170e` |
| MagiK source | `d903ea217a506eedb5b818f3e15b704b6bad6d8c` |
| GUI | `52a79119510150f668887f5c636611b0659f3783cdee0273d6da45b75cba62f0` |
| Main | revision `f290719e97f5a3c84efa8e24691b80673b93f23c`, SHA-256 `17c2c9fcb3c62bc2831d0de27ba7234eed1037e4e9fe754b0b4d09fd609dbda7` |
| Scanout module | `910d81ac03467e58ee93e48811dfb28e2fff21012192fb658c288c9dc2f50003` |
| Latch RBF | `9f7fdd78041bf11638618f51e243157ed33db259081b283f1e90b21738c1192f` |
| Report branch | `nigel/production-performance-review-2026-08-23-qualified` |

The catalog authority rule is stricter than the generic benchmark policy:

- only a catalog purged before a supervised reboot, then built from scratch on
  the new boot, is timing authority;
- warm catalog runs are correctness or attribution diagnostics only, even when
  the benchmark itself reports a clean result;
- pprof, PMU, tracefs, and Streamline runs are attribution-only;
- protocol-v5 physical repeated-vblank counters, not latch-post drops, are the
  cadence authority.

The final authoritative catalog run is
`build/agent-benchmarks/cold-boot/1787508360`. Its post-completion diagnostics
and integrity proof are retained beneath that directory. The profiled cold run
is `build/agent-benchmarks/cold-boot-pprof/1787508191` and is explicitly not a
timing control.

### Campaign outcome

The campaign covered the principal production journeys and the nonredundant
profilers needed to explain them. Experimental effects, effect comparisons,
compatibility aliases, fault-obstruction labs, and the attended
`launch-return-once` incident route were not run.

| Group | Result |
|---|---|
| Input integrity | `input-integrity` passed; zero loss, duplication, sequence gaps, proxy failures, journal overflows, or latch drops |
| Modal input | `modal-input` passed; held/released input did not leak through the modal and a fresh press entered Arcade |
| Launcher response | Failed the latency gate while integrity passed |
| GUI cadence/composition | `settled-composition`, `settings-navigation`, `arcade-velocity-scroll`, `bridge-model-churn`, and `gui-frame-attribution` passed |
| Search/system entry | `search`, all 69-system `system-entry`, and 60-sample `system-entry-critical-confirm` passed |
| Catalog/storage helpers | `catalog-corpus-inventory`, `preview-work-attribution`, `rom-identity-hashing`, and `media-pack-persistence` passed |
| Cold catalog | Final reboot-first, purge-first run passed through first presentation and later produced a valid complete V3 generation |
| Warm catalog controls | `catalog-resume-validation` passed as diagnostic correctness evidence; build/rebuild harness attempts were invalid and are not timing evidence |
| Attribution | `gui-frame-attribution`, `scheduler-trace`, `pmu-profile`, `storage-attribution`, `neon-attribution`, and fresh-catalog `cold-boot-pprof` completed |
| Launch/return | Failed before timing: no new return state was written within 20 seconds; canonical diagnosis restored/proved healthy Home state |

`launcher-response-attribution` timed out at 28 of 33 confirmations and wrote no
artifact. It is an invalid profile attempt, not performance evidence. The
ordinary launcher-response control remains valid. `catalog-build-rebuild`
expired during its automation session, and `catalog-full-build-rebuild` failed
its updater-index prefetch evidence gate. Both were warm and are excluded from
all timing conclusions. The successful warm resume run proves ordinary restart
identity and resume behavior only.

The standalone system-entry qualification matrix, redundant per-route pprof and
Streamline wrappers, and launch-return attribution were not continued after
their corresponding 69-system/60-sample controls, combined GUI attribution, or
base launch-return gate had already resolved the decision. This avoids treating
observer overhead or repeated SD activity as additional independent evidence.

### Authoritative reboot-first catalog result

The final control purged the Dev catalog, changed boot ID from
`543965bb-9f1c-4391-bc3f-5a7d31922dc0` to
`9c8a64d1-f6f0-449d-9cde-4f16b767262e`, and entered the
`cold_no_catalog` path. The first-frame benchmark reported
`timing_authoritative=true`. The same build was then observed to terminal
publication through typed diagnostics and validated with typed V3 inspection.

| Milestone | Boot-relative | MagiK startup-clock relative | Evidence |
|---|---:|---:|---|
| Linux agent start | 6.890 s | — | cold summary |
| Initial Main entry | 8.083 s | — | cold summary |
| MagiK process start | 11.367 s | — | cold summary |
| Catalog worker start | about 14.624 s | 1.189 s | `catalog_worker_start` |
| Arcade first-visible scan complete | about 25.530 s | 12.095 s | 1,150 discoveries, 2,987 normal files |
| Arcade first-visible ready | about 25.580 s | 12.146 s | 922 games |
| Production intro complete | about 36.022 s | 22.587 s | 1,198 frames |
| First physical Home frame | 36.980 s | 23.545 s | physically confirmed, capture verified |
| Full 161-target scan complete | 144.270 s | 130.830 s | 51,693 discoveries, 52,706 normal files |
| Authoritative 40,013-game catalog prepared | 164.350 s | 150.913 s | resident Arcade bootstrap retained |
| V3 manifest generation 1 published | 199.080 s | 185.647 s | all 69 systems rebuilt |
| Builder persisted | 204.980 s | 191.547 s | builder elapsed 190.316 s |

The completed progress record reports 190.440 s wall elapsed, 190.439 s active,
49 ms intentionally inactive, 161/161 targets complete, and no running worker.
Typed inspection then proved:

- V3 generation 1 valid;
- 69 systems and 40,013 visible games;
- 51,693 persisted discoveries;
- 69 NavPacks totalling 13,932,140 bytes;
- 922 resident Arcade games;
- consistent identity, ordering, launch, search, and artifact-set hashes.

The full scan consumed 118.489 s. Its overlapping internal counters report
105.561 s discovery and 112.218 s classification, so those values must not be
added. Preparing the full catalog completed at boot second 164.35; shard build,
publication, and manifest work then consumed 34.73 s before manifest
publication, followed by 5.90 s to the final persisted event.

Cold startup cadence failed even though the launcher eventually became healthy:
the production intro recorded 45 physically reused refresh intervals, four
pacing failures, and a maximum confirmation gap of 216.651 ms. This is not a
latch-post-drop inference; it comes from the authoritative startup cadence
record. The first physical Home frame consequently arrived 11.40 s after the
first-visible catalog was already ready.

### Cold-start CPU attribution

The reboot-first pprof arm was observer-perturbed and correctly reported
`timing_authoritative=false`. It also exposed a different first-visible outcome
of zero games, which is further reason not to mix its elapsed times with the
control. Its 7,039 CPU samples are still useful for attribution:

| Category | Samples | Share |
|---|---:|---:|
| Production startup intro renderer/raster | 3,212 | 45.6% |
| Catalog threads/functions | 1,137 | 16.2% |
| FPGA status/capture operations | 452 | 6.4% |
| Launcher readiness, salience, and snapshot work | 436 | 6.2% |

The dominant individual leaves were the production `IntroScene` render path,
point-command rasterization, MiSTer formation rendering, catalog work,
MRA-byte inspection, and RGB565 readiness evidence. The conclusion is not to
tune experimental effects; it is to reduce the production startup intro's
per-frame work and its interference with first catalog construction.

### Input response and scheduler evidence

`launcher-response` failed only the response gate:

| Metric | Result |
|---|---:|
| Dispatch P95 / max | 15.256 / 18.201 ms |
| Physically confirmed median | 20.156 ms |
| Physically confirmed P95 / max | 33.624 / 37.674 ms |
| Lost / duplicated / coalesced / reordered actions | 0 / 0 / 0 / 0 |
| Journal overflows / sequence gaps / proxy failures | 0 / 0 / 0 |
| Latch drops / ownership losses | 0 / 0 |

The repeated-vblank count in this mixed 60/50 Hz static-route test is
diagnostic, not a drop failure. The important separation is that input data is
delivered exactly once, but UI dispatch and physical confirmation are too slow.

The diagnostic scheduler trace covered 7.773 s with zero trace overruns:

- CPU0 was 15.4% busy and CPU1 was 42.5% busy;
- both cores were simultaneously busy for only 7.94% of the interval;
- the input-reader P95 runnable delay reached 18.861 ms in one arm and 6.792 ms
  in another;
- the vsync thread P95 runnable delay reached 28.639 ms;
- preview workers remained small and mostly on CPU0;
- no migration was observed for the principal UI and input threads.

This confirms adequate aggregate capacity and generally correct affinity, but
poor worst-case service on the latency-sensitive CPU1 path. It argues for
shortening/nonblocking CPU1 work and prioritizing input adoption before raster
or completion polling. It does not justify a general pool, sustained CPU1
catalog work, or indiscriminate additional real-time threads.

The ordinary `launch-return` run failed before a timing artifact was produced:
the launcher did not write a new return state in 20 seconds. The immediate
canonical diagnosis reported a healthy `LauncherActive` Home state with no
temporary repair and no reboot. Launch/return therefore remains a correctness
investigation, not a performance optimization result.

### GUI cadence, rendering, and bridge evidence

The 40-second unprofiled Arcade held-scroll control was clean:

- 59.9988 Hz physical refresh and 60.0005 submitted FPS;
- 2,411 owned/presented vblanks;
- zero repeated physical vblanks, dropped frames, latch drops, ownership
  losses, and sequence gaps;
- foreground work P95 4.268 ms, P99 5.233 ms, maximum 6.049 ms;
- completion interval median 16.657 ms, P99 17.518 ms, maximum 18.028 ms.

All 12 landscape and portrait-left Settings navigation legs also passed with
zero physical drops, latch drops, sequence gaps, or semantic snapshot
violations. Some legs reached roughly 13.6 ms maximum completion-poll wall time,
which keeps polling as a measured P2 opportunity but not a current cadence
failure. Settled composition passed its receipt-scoped modal retirement,
terminal pixel, and cadence gates.

Bridge churn proved that retained light synchronization already works:

| Workload | Replacements | Row allocations | Row mutations | `SharedString` constructions | Bridge+raster per update |
|---|---:|---:|---:|---:|---:|
| 64 light updates | 0 | 0 | 0 | 0 | 7.601 ms |
| 60 media-progress updates | 1 | 30 | 26 | 185 | 1.375 ms |
| 64 menu-selection updates | 2 | 256 | 126 | 384 | 8.306 ms |

Raster, not bridge synchronization, dominates the light and menu paths. This
rejects broad model replacement work and keeps row allocation/data layout as
the narrower target.

The combined GUI attribution arm retained a clean unprofiled control with zero
physical repeated vblanks, latch drops, ownership losses, or sequence gaps.
The largest PMU span totals were:

| Span | Cycles | Data-stall ratio | L1D refill ratio | NEON instruction share |
|---|---:|---:|---:|---:|
| Arcade overlay copy | 60.69 M | 43.5% | 11.9% | 39.4% |
| Ordinary Slint raster | 33.41 M | 18.4% | 3.7% | 0.16% |
| Custom-layer generation | 29.25 M | 26.8% | 6.9% | 0.71% |
| Arcade row update | 22.15 M | 29.4% | 7.9% | 0.83% |
| Latch post request | 20.04 M | 7.3% | 2.8% | 0.37% |
| CRT Arcade-list update | 18.15 M | 33.6% | 9.7% | 0.97% |

The dedicated NEON profile confirms the policy rather than reopening it. In
landscape, Arcade overlay copy had a 39.6% NEON share and 44.8% data-stall
ratio. In portrait-left, custom-layer generation had a 15.8% NEON share and
46.9% data-stall ratio, while overlay copy had a 35.6% NEON share. Arcade row
and CRT list updates remained almost entirely scalar and cache-stalled. Broad
new SIMD is therefore lower value than allocation, row-cache, and memory-layout
work. A contiguous-copy experiment remains gated on a route-specific cycle
share, pixel parity, and unprofiled cadence improvement.

### System entry, search, preview, and media

The all-system entry control passed for 69 populated systems and 40,013 games.
The critical confirmation repeated C64, SNES, PC88, NES, BBC Micro, and Arcade
ten times each in fresh launcher processes with no failures. Complete-ready P95
was 41–43 ms for four systems and 69 ms for SNES and Arcade. NavPack open P95
was only about 0.4–0.6 ms. First-frame list projection, raster, and confirmation
dominate; broad NavPack prewarming is rejected as a first-line optimization.

Search passed three suites over four queries and 20 iterations. Warm total P95
was 25.071 ms and maximum 25.551 ms; SQLite open P95 was 0.668 ms, statement
prepare P95 2.347 ms, and execute/materialize P95 21.124 ms. The broad `2 player`
query returned 797 matches and dominated. Across direct timing there were 264
opens, 528 prepares, and no retained worker. The UI's broad `A` query became
ready in 281 ms with 922 resident-catalog results and zero persisted workers.
Retaining connections or workers is therefore not the first target; bounded
result materialization/ranking is the only search item worth reopening.

Preview attribution processed 500 requests with zero collisions, duplicate
reads, duplicate decodes, duplicate resizes, or repeated missing-sidecar probes.
The duplicate-work opportunity gate was false. Decode consumed 712.760 ms total
versus 69.153 ms reads. This rejects new deduplication/cancellation machinery;
compressed-input scratch reuse remains only a low-risk allocation cleanup.

Media persistence passed nine production-pack arms with one exFAT writer,
identity/raw MMLZ4B input, no decode staging, and no tmpfs use. Total-flow rates
were about 41–51 Mb/s for the 4.97 MiB NeoGeo pack, 57.7–59.3 Mb/s for the
24.3 MiB Arcade pack, and 49.0–63.4 Mb/s for the 47.7 MiB Amiga pack.
Save/publish cost ranged from 17–32 ms, 60–70 ms, and 112–148 ms respectively.
The serial direct-to-exFAT writer policy should remain.

ROM identity hashing was deterministic across three runs, used streaming
slicing-by-eight CRC with one fixed 256 KiB buffer, and performed no whole-file
allocation. A diagnostic 32 MiB N64 sample spent about 1.27 s in CRC, but the
production default is Lynx-only; metadata discovery and selection consumed more
of the ordinary workload. Hashing is not a broad optimization target.

### Catalog corpus, Cortex-A9 PMU, and exFAT attribution

Corpus inventory passed 161 targets. Representative walks were:

| Target | Candidates | Directories | Time | Namespace backend |
|---|---:|---:|---:|---|
| Arcade | 3,004 | 558 | 2.285 s | fd-relative |
| SNES | 1,864 | — | 0.147 s | fd-relative |
| C64 | 18,915 | 67 | 13.361 s | fd-relative |
| PC88 | 3,926 | 2,038 | 11.562 s | fd-relative |

All 161 targets used the fd-relative backend without fallback or restart. This
closes the current-corpus double-walk fallback hypothesis.

The warm PMU catalog suite is attribution-only. Aggregating its fixed
fresh/rebuild/rebuild-all operations, the largest recorded spans were scan
execution/walking, scan/classification, persistence, and catalog prepare. Major
aggregate examples were:

| Span | Cycles | IPC | L1D refill ratio | Branch-mispredict ratio |
|---|---:|---:|---:|---:|
| Execution walk | 65.76 B | 0.60 | 1.83% | 12.0% |
| Scan | 62.50 B | 0.75 | 1.43% | 13.2% |
| Persist | 55.01 B | 0.57 | 2.91% | 19.5% |
| Prepare | 30.06 B | 0.71 | 1.64% | 25.0% |
| Search-index shard work | 14.19 B | 0.67 | 1.88% | 19.3% |
| Validate shard work | 7.77 B | 0.60 | 2.44% | 24.5% |
| Search-row materialization | 7.00 B | 0.58 | 2.81% | 24.7% |
| Game projection | 5.88 B | 0.49 | 3.70% | 34.1% |

The generally low-to-moderate IPC and substantial branch/cache pressure argue
against simply moving more catalog work to CPU1. They support reducing items,
allocations, copies, and synchronization first.

The warm storage trace is also attribution-only, but it directly characterizes
the exFAT path:

- filesystem `/media/fat` was exFAT with `rw,noatime,nodiratime`;
- 194.2 MB was read and 108.7 MB written at block level;
- average block wait was 38.65 ms for reads and 14.35 ms for writes;
- maximum in-flight block requests was two;
- the process performed 36,858 read and 19,931 write syscalls;
- namespace production took 42.82 s, consumer-active work 53.16 s,
  consumer wait 9.18 s, and channel wait 13.89 s;
- 16,877 synchronous sends and 11,649 namespace buffer allocations were
  recorded, with peaks of 5,963 entries and 978,524 bytes;
- all 161 targets remained fd-relative with zero fallback/restart;
- the final isolated catalog was valid with 69 systems and 40,013 games.

The absolute elapsed values are warm and not catalog authority. Their relative
attribution is strong enough to prioritize fewer handoffs and less write/copy
volume, while rejecting more concurrent exFAT writers.

### Local artifact index

The benchmark tree is ignored delivery evidence rather than source, but these
paths identify the exact local captures used for this report:

| Evidence | Artifact directory |
|---|---|
| Input integrity | `build/agent-benchmarks/input-integrity/1787503754` |
| Modal input | `build/agent-benchmarks/modal-input/1787503799` |
| Launcher response | `build/agent-benchmarks/launcher-response/1787503825` |
| Settled composition | `build/agent-benchmarks/settled-composition/1787504336` |
| Bridge churn | `build/agent-benchmarks/bridge-model-churn/1787504362` |
| Settings navigation | `build/agent-benchmarks/settings-navigation/1787504389` |
| Arcade velocity scroll | `build/agent-benchmarks/arcade-velocity-scroll/1787504447` |
| System entry, all systems | `build/agent-benchmarks/system-entry/1787504544` |
| System-entry critical confirm | `build/agent-benchmarks/system-entry-critical-confirm/1787504818` |
| Catalog corpus inventory | `build/agent-benchmarks/catalog-corpus-inventory/1787505041` |
| Preview work attribution | `build/agent-benchmarks/preview-work-attribution/1787505120` |
| ROM identity hashing | `build/agent-benchmarks/rom-identity-hashing/1787505208` |
| Media-pack persistence | `build/agent-benchmarks/media-pack-persistence/1787505258` |
| Warm resume validation | `build/agent-benchmarks/catalog-resume-validation/1787505487` |
| GUI frame attribution | `build/agent-benchmarks/gui-frame-attribution/1787506723` |
| Scheduler trace | `build/agent-benchmarks/scheduler-trace/1787506806` |
| Cortex-A9 PMU suite | `build/agent-benchmarks/pmu-profile/1787506871` |
| exFAT storage attribution | `build/agent-benchmarks/storage-attribution/1787507355` |
| NEON attribution | `build/agent-benchmarks/neon-attribution/1787507608` |
| Fresh-catalog cold pprof | `build/agent-benchmarks/cold-boot-pprof/1787508191` |
| Authoritative fresh-catalog cold control and completion proof | `build/agent-benchmarks/cold-boot/1787508360` |

## Evidence-driven optimization plan

### P0 — batch the walker/indexer contract, preserving target atomicity

The authoritative cold scan is 118.5 s of a 190.3 s builder. The diagnostic
storage run recorded 16,877 synchronous sends, 13.9 s of channel wait, and
11,649 buffer allocations. Current code sends one `DiscoveryEvent::File` per
candidate and the consumer immediately wraps it in a one-element vector.

Prototype a bounded batch event of 32–64 files, flush at target boundaries and
cooperative parking points, and retain reliable terminal/error events. Keep the
walker on CPU0 during interaction and preserve target-level restart semantics.
Do not replace this with a broad work-stealing pool.

Acceptance requires at least three purge-before-reboot cold controls for each
candidate: identical catalog hashes/counts, zero resume/publication regression,
no first-visible regression beyond run noise, lower scan and full-persist
median, and clean input/Arcade cadence. A useful implementation threshold is at
least 15% lower scan time or 10% lower end-to-end builder time.

### P0 — fix production cold-intro cadence and CPU interference

The unprofiled cold control dropped 45 physical refreshes and the pprof arm
attributed 45.6% of samples to the production intro renderer/raster. First
visible was ready at boot second 25.58, but Home was not physically presented
until 36.98.

Reduce point-command generation/raster work, cache static formation data, and
avoid rebuilding salience/readiness inputs per frame. Keep the visible design
and exact pixel contract; this is production startup work, not an experimental
effect comparison. The gate is zero physical dropped frames on repeated
reboot-first controls with no later first-visible or full-catalog regression.

### P0 — shorten CPU1 input adoption latency

Input delivery is exact, but dispatch P95 is 15.3 ms against a 3 ms gate and
physical confirmation P95 is 33.6 ms. Scheduler attribution shows long runnable
tails on input and vsync despite low dual-core overlap.

Instrument and then shorten the poll-return → publish → UI-drain → state-apply
segments. Drain pending input before raster/completion polling, avoid unnecessary
unchanged atomic work-mode exchanges, and ensure the UI's real-time section
blocks/yields promptly rather than occupying CPU1 through noncritical work. Do
not add sustained CPU1 catalog work or broadly promote threads to real-time.
Retest at both 50 and 60 Hz using the existing <=3 ms P95 / <=5 ms max dispatch
gates plus zero integrity failures.

### P1 — reduce shard publication and write/copy volume

The cold build spent 34.73 s between authoritative catalog preparation and
manifest publication, then another 5.90 s reaching persisted. PMU persistence
was a 55.0-billion-cycle aggregate span, and the warm storage arm wrote 108.7
MB. Preserve the single writer and atomic manifest contract, but evaluate:

- fusing artifact copy and hash passes where the same bytes are currently read
  twice;
- content-addressed no-op publication for unchanged shards;
- persisting media-pack identity in generation metadata so a current pack does
  not trigger full shard/sidecar/NavPack/SQLite reconciliation;
- eliminating repeated Arcade ROM-inventory construction within one operation.

Each change needs interruption/resume integrity plus reboot-first full-build and
incremental controls. Never trade manifest durability for a warm microbenchmark.

### P1 — remove Arcade row/filter allocation without disturbing cadence

Arcade already sustains 60 Hz with zero physical drops, so this is headroom and
latency work rather than a visible cadence rescue. Cache filter rows, write
RGB565 directly into retained row storage, and replace temporary `Pixel`,
`Vec<u16>`, and changed-row allocations with bounded reused storage. Preserve
the current custom-layer/damage contract. Require pixel parity, the 40-second
held-scroll gate, both orientations, and no input-response regression.

### P1 — coalesce progress and reduce cross-core ownership traffic

The progress channel can outpace the UI, which consumes at most two events per
frame, and numeric progress is emitted for 256 KiB copy chunks. Replace queued
numeric progress with a latest-value slot while keeping start, error, terminal,
and persisted events reliable and ordered. Add an unchanged-value early return
to catalog work-mode updates. Validate resume identity and make queue depth,
coalescing, and terminal latency explicit counters.

### P2 — targeted cleanups only

- Move preview filesystem metadata calls outside the shared cache mutex.
- Reuse the preview compressed-input buffer, but do not add deduplication or
  cancellation: the 500-request gate found no duplicate work.
- Investigate completion polling only where the existing profile exceeds 10 ms;
  any backoff must preserve 60 Hz and input confirmation.
- Tune resume flush/barrier frequency only with reboot-first fault-safe evidence;
  warm resume timing is not authority.
- Bound broad search result materialization/ranking if interactive prefixes are
  optimized; do not retain connections merely to save sub-millisecond opens.
- Consider adjacent NavPack page prefetch only for a measured page-fault tail;
  current opens are sub-millisecond and broad prewarm is rejected.
- Keep existing NEON copy routes. Add no new SIMD path without >=10% eligible
  cycle share, exact pixel parity, and a clean unprofiled cadence win.

### Rejected changes

- No general thread pool and no sustained background work on CPU1.
- No increase in concurrent exFAT writers or tmpfs staging for production media.
- No namespace fallback rewrite for the current corpus: 161/161 targets used
  fd-relative walking with no fallback.
- No preview deduplication/cancellation framework: duplicate work was 0%.
- No broad NavPack prewarm: open/mmap/validation is not the entry bottleneck.
- No persistent search worker solely to avoid open/prepare cost.
- No weaker shard validation, manifest ordering, parent sync, or resume
  durability.
- No conclusions from warm full-build elapsed times, latch-post drops, or
  profiled frame cadence.

## Recommended implementation order

1. Add the cold-catalog comparison protocol and batch walker handoff.
2. Reduce production intro render work until reboot-first cadence is clean.
3. Resolve CPU1 input adoption latency with segment-level evidence.
4. Fuse or eliminate redundant shard publication I/O while retaining durability.
5. Remove Arcade row/filter allocation and unchanged atomic/progress traffic.
6. Re-run the full production journey matrix, including launch/return after its
   correctness failure is diagnosed.

The most important architectural conclusion is that MiSTer MagiK is not short
of aggregate dual-core capacity. It is losing time in serialized namespace and
catalog work, branch/cache-heavy projection and persistence, long-tail CPU1
service, and a production startup renderer that competes with the cold build.
The optimization strategy should reduce work and ownership transfer, keep CPU1
predictable, and minimize exFAT bytes and sync points rather than adding
parallelism.
