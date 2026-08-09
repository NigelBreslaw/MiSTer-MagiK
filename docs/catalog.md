# Catalog V3

Catalog V3 is the only production catalog used by MiSTer MagiK. Its public
registry, state, binding, and scanner-cache schemas are version **1**; the
mini-navigation schema is version **2** and the SQLite shard schema is version
**4**. There is no legacy read
fallback, migration bridge, dual publication, global summary, or global
navigation file.

The catalog is split by playable system (`arcade`, `snes`, `c64`, and so on),
not by launcher presentation groups such as Nintendo or Sega. A system is the
smallest independently selectable, rebuildable, and lazily loadable unit. The
checked-in taxonomy may group those systems differently without rewriting
catalog storage.

## Goals

- Reveal a useful UI as soon as Arcade is available on a first build.
- Start warm launches from a small registry and one eager Arcade mini-nav.
- Load other systems only when selected.
- Rebuild and publish only changed system projections.
- Keep the published generation launchable while warm reconciliation prepares
  its replacement.
- Report queued, scanning, prepared, and failed activity per system rather than
  treating the whole catalog as one binary scan.
- Keep every post-reveal scan and rebuild phase continuously on CPU0 at
  background priority so UI work on the display core remains responsive.
- Keep catalog construction testable and benchmarkable without Slint, the
  framebuffer, Main, or a complete MiSTer installation.

## On-Disk Contract

The production root is `/media/fat/mister-magik/catalog-v3` (or the matching
development layout). `MISTER_SHARDED_CATALOG_DIR` may override it for tests.

```text
catalog-v3/
  registry/
    manifest-a.json
    manifest-b.json
  systems/
    arcade/<generation>.sqlite3
    arcade/<generation>.nav.lz4b
    snes/<generation>.sqlite3
    snes/<generation>.nav.lz4b
    ...
  state/
    catalog-state.sqlite3
    scanner-cache.sqlite3
    builder-state.sqlite3
    build-progress.sqlite3
  catalog.binding.json
```

The two manifest slots provide an atomic registry commit. Each manifest names
the active and, where retained, previous immutable SQLite/navigation pair for
every system, including sizes, hashes, metadata, generation, and game count.
Readers choose the newest completely valid slot. Partially written generations
are unreachable.

`catalog.binding.json` binds the active registry generation and the
`rich-game-v2` projection contract to the canonical fingerprint in
`state/catalog-state.sqlite3`. Publication writes shards, the manifest, and the
binding before replacing catalog state. An interruption may cause a conservative
rebuild, but cannot make incomplete artifacts authoritative.

Cold-build persistence retains only compact catalog state and scan statistics.
The complete filesystem scan is released before shard publication; the RAM
catalog is released before scanner-cache staging. Scanner-cache staging is
deliberately sequential with shard publication so their peak allocations do
not overlap. On Linux, those explicit lifetime boundaries also return wholly
free glibc arenas to the kernel so a completed phase cannot inflate the next
phase's RSS.

On MiSTer, each bounded system shard is constructed and validated under
`/tmp/mister-magik/catalog-v3-build` when the materialized rows have a
conservative amount of free tmpfs headroom; otherwise that shard falls back to
the catalog's on-media staging directory. During a fresh build, one producer
constructs the next shard while one publisher copies the preceding shard to the
SD card. The one-entry handoff admits at most two in-flight shards, and capacity
is checked again while the publisher's tmpfs allocation remains charged. If the
next shard does not fit, publication drains before sequential on-media staging
begins, avoiding simultaneous SD read/write contention. Incremental and
replacement rebuilds remain sequential.

Publication computes artifact hashes during the single bounded RAM-to-SD copy,
writes same-filesystem temporary files on `/media/fat`, renames them into their
immutable generation paths, and retains the existing artifact barrier and
manifest-last commit. This keeps normal SQLite page creation, indexing,
validation reads, and separate checksum passes off the SD card while preserving
a two-system fresh-build lifetime bound. Because tmpfs pages consume RAM,
first-scan qualification still gates the process-wide peak HWM independently of
Rust heap retention.

`state/scanner-cache.sqlite3` separately owns discovery timestamps and software
hashes. It is scanner state, not a game catalog or UI projection.

`state/builder-state.sqlite3` owns the last successfully committed scan-unit
fingerprints and their produced-system dependencies. Stable scan-unit IDs are
derived from target kind and normalized path. The planner expands changed
inputs through those dependencies to select exact systems; missing, corrupt,
ambiguous, schema-incompatible, or manifest-mismatched state conservatively
selects all published systems. Planner state is committed only after the new
manifest, binding, scanner cache, and catalog state are durable.

`state/build-progress.sqlite3` is an interruption journal for an in-progress
initial or warm build. It is bound to the active and intended generations,
semantic contract, and target fingerprints. Matching completed targets and
prepared shards can be resumed after launching a game terminates the launcher;
changed targets invalidate only their dependent checkpoints. A manifest or
semantic-contract change discards the journal. Both builder state databases are
disposable internal state and are never publication authority.

The retired files `library.sqlite3`, `library.summary.json`, and
`library.nav.lz4b` are not production inputs or outputs. Acceptance fails if a
current build recreates them.

## Startup And Lazy Loading

Warm startup reads the registry first. This supplies system titles, placement,
ordering, counts, and immutable artifact references without opening every
system database. Arcade's mini-nav is opened eagerly because it is the first
visible game collection. Other system shards stay closed until predictive Home
hydration requests them.

Home navigation predictively schedules bounded background mini-nav loads for
the highlighted destination (or the default leaf below a highlighted submenu)
and a small number of nearby, bounded-size siblings. The sibling count and game
count limits protect Home frame time and memory on the MiSTer. Loaded rows and
structured launch plans are merged into a complete replacement catalog on the
existing shard worker. The launcher validates the catalog version and adopts
that prepared value with a cheap swap; it never projects rows or rebuilds
catalog indexes on the UI thread. Immutable indexes are shared between catalog
snapshots, so worker handoff and empty taxonomy placeholders are cheap. A stale
prepared result is discarded and requested again against the current version.
Loaded rows remain resident.
Active lazy hydration is visually silent: destination tiles keep their normal
ready and focus presentation unless the shard load actually fails. Activating
a collection is atomic: the launcher stays on the populated source view until
the destination has resident rows, then changes screen and publishes those rows
in one frame. A failed shard load is surfaced once on the destination tile and
does not retry every frame;
pressing A on that tile explicitly retries within the current catalog generation.

The following numbers are deliberately distinct:

- total registered games: sum of all active system counts in the registry;
- registered systems: active system entries in the registry;
- resident Arcade games: eagerly loaded Arcade rows;
- selected-system games: rows loaded lazily for the current non-Arcade system.

Resident Arcade rows must never be reported as the total catalog.

## First Build And Progressive Reveal

With no valid registry, the launcher first probes the bounded
`arcade-bootstrap.nav.lz4b` index beside `catalog-v3`.
The index is an exact, checksummed Arcade mini-nav plus its source stamp. A
matching index restores the canonical Arcade rows and structured launch plans
without walking `_Arcade` or opening every MRA. Missing, corrupt, oversized, or
stale indexes enter the first-run particle intro and fall back to a CPU0 Arcade
scan. When that bounded projection is acknowledged, the launcher can prepare
Home and Arcade off screen.
System discoveries update scanning tiles while the complete authoritative scan
continues. These progressive discovery tiles are presentation-only placeholders:
they cannot schedule a system-shard read until a valid registry generation is
authoritative. Publishing that registry reconciles and removes non-authoritative
placeholders before lazy hydration or taxonomy synchronization can observe the
new generation.

The retained index is generated locally from each MiSTer's canonical catalog;
it is not a catalog of one developer's paths and is not assumed to match every
Update All installation. Different game sets and files therefore receive their
own exact index. The standard `/media/fat/_Arcade` root remains the production
Arcade bootstrap input.

After the first-visible boundary, the build becomes background work. The full
catalog is projected into per-system immutable artifacts and committed through
the registry. Only the final complete generation is catalog authority. The
bootstrap index is a disposable startup accelerator: it is published atomically
after a valid first-visible snapshot, refreshed from the completed full catalog,
and never used to suppress the authoritative background scan. That scan audits
weak filesystem stamps and replaces any initially stale projection during the
same launch.

The standalone catalog-builder command retains its foreground-through-first-
visible policy. The embedded cold launcher instead selects
`BackgroundContinuous` before bootstrap begins. That explicit execution policy
prevents the builder from silently changing its coordinator or helper threads
back to all-core foreground roles while the intro owns CPU1.

## Continuous CPU0 Background Work

During the first-run intro, and after any first-visible catalog becomes usable,
the catalog parent uses the `CatalogWorker` policy (nice 5, CPU0). The walker
uses the lower-priority `LibraryWalker` policy (nice 10, CPU0). Audit,
projection, snapshot, shard construction, publication,
scanner-cache, catalog-state, and helper threads remain explicitly pinned to
CPU0 or inherit that affinity.

Input, navigation motion, media work, preview work, scripted benchmarks, and
visual animation never pause catalog construction. Cooperative checkpoints
remain at bounded cancellation and scheduling boundaries, but their permission
stays open. Checkpoints cover:

- builder and scan-plan boundaries;
- library and prepared-payload filesystem walks;
- candidate classification batches;
- archive inspection;
- collection and coverage processing;
- deferred projection and publication preparation.

Checks occur at bounded batches rather than by polling an atomic for every
file. A checkpoint must not hold a filesystem iterator borrow, database
transaction, publication lock, or UI lock.

The embedded launcher applies CPU0 confinement at the builder-start boundary;
it does not depend on successful first-visible publication to make that
transition. Standalone administrative builds remain all-core and normal
priority until their first-visible snapshot is published and retained.

Initial and warm builds keep disposable durable progress in
`catalog-v3/state/build-progress.sqlite3`. Completed scan
targets are committed atomically with their eligible-input fingerprints. After
a launcher handoff terminates MagiK, the next launcher re-enumerates target
metadata, hydrates exact matches without reparsing or classifying them, and
continues with new or changed targets under the same build ID. Scan outputs
are committed in bounded groups of at most 16 targets or 2 MiB of encoded
output, whichever comes first. This preserves atomic resume boundaries without
paying an exFAT durability barrier for every small directory. Completed,
unpublished system shards are likewise hash- and schema-checked before reuse.
Resumable first-build shard publication is deliberately sequential: each shard
is synced, validated, and journaled before the next begins. Warm replacement
rebuilds remain background work and retain published authority while candidate
shards are prepared.

Neither the latch nor the progress journal is catalog authority. Readers accept
only the normal publication chain: all immutable shards, artifact barrier,
complete manifest, catalog binding, scanner cache, and catalog state. The
journal is removed last after that chain succeeds; missing, corrupt, or
contract-mismatched recovery state is discarded and rebuilt.

The catalog crate owns only a generic cooperative-work permission signal and
background scope. It does not depend on Slint or launcher types. The UI runner
adapts the latch into that signal.

## Incremental Rebuild And Publication

The worker vocabulary is `LoadOnly`, `CheckStamp`, `InitialBuild`,
`Reconcile { scope: ChangedInputs | AllSystems }`, and `FreshBuild`. A missing
catalog always becomes an initial build. `ChangedInputs` uses committed
scan-unit dependencies to select exact systems. `AllSystems`, used by Settings
→ **Rebuild Database**, deliberately rebuilds every published/current system
without deleting the active generation.

The scanner computes the canonical catalog stamp and uses its scanner cache and
builder state to avoid unnecessary source work. Projection reconciliation
compares each candidate system with its currently published shard. Unchanged
systems retain their immutable artifact paths and generations; changed systems
receive new SQLite/navigation pairs; removed systems disappear only from the
next published registry.

If no system changes, no new registry generation is published. If one system
changes, only that system projection is rebuilt even though the registry still
describes the complete catalog. The work ratio and measured elapsed speedup are
reported separately. A 10x result is a useful target and historical comparison,
not a release blocker; selection correctness and UI responsiveness are gates.

Publication is manifest-last and failure-atomic. Garbage collection may remove
only artifacts not referenced by the active or retained previous generation.

## Published Availability And Update Activity

Published availability and update activity are independent. The launcher
projects each system as:

- published plus queued/scanning/prepared: selectable with its published game
  count and old games;
- new plus queued/scanning/prepared: a disabled scanning placeholder;
- published plus failed: selectable with an update warning;
- new plus failed: disabled with a hard failure;
- unaffected: unchanged.

Parent groups aggregate only updating descendants. Prepared shards remain
non-authoritative and continue to display as scanning. `ManifestPublished` is
the final builder event; only then does the launcher refresh the registry,
replace an active changed collection, remove deleted systems, or return Home
when the active collection was removed. A warm failure keeps the old generation
and screenshot media available.

## Compatibility And Recovery

The projection contract and shard schema identify generated catalog formats,
not user content. A valid `rich-game-v1` binding or schema-two shard is an
upgrade requirement, not corruption. Reconciliation rebuilds every affected
system into schema-four, `rich-game-v2` immutable artifacts even when its game
fingerprint is unchanged. Incompatible resumable build journals are discarded.
The previous manifest remains authoritative unless every replacement shard,
the new manifest, binding, scanner cache, and catalog state validate and publish
successfully.

An atomic rebuild retains the usable catalog while replacements are built and
switches authority with the normal manifest-last publication. Settings →
**Rebuild Database** is a full warm `AllSystems` reconciliation: it does not
delete the active catalog or screenshot packs, stop media work, reboot, or show
the blocking first-build overlay.

Fresh recovery may remove generated catalog artifacts before constructing a
new cold generation. The attended production command
`mister-magik-fb purge-library-data --confirm` is broader: it deletes legacy
database artifacts, Catalog V3, the Arcade bootstrap index, supported
screenshot packs and temporary variants, sidecar/media state covered by the
reset matcher, reports separate catalog and screenshot counts, and does not
reboot. It preserves ROMs, configuration, saves, unrelated assets, and
unsupported files. Without the exact `--confirm` argument it prints usage and
performs no mutation.

Catalog decisions use one lifecycle-owned dialog after the scan overlay has
been cleared:

| Situation | Left/default | Right | B/Home |
| --- | --- | --- | --- |
| Format upgrade with usable catalog | Continue | Rebuild atomically | Continue |
| Rebuild or persistence failure with usable catalog | Continue | Full rebuild | Continue |
| Transient failure without a catalog | Retry | Full rebuild | Exit to MiSTer |
| Corrupt or deterministic failure without a catalog | Exit to MiSTer | Full rebuild | Exit to MiSTer |

Continuing preserves resident/bootstrap Arcade entries and any other already
available systems. A lazy-load failure marks only that system unavailable and
does not block the launcher. Deterministic schema mismatches do not offer Retry.

Arcade metadata comes from the commit-pinned ArcadeDatabase rows embedded in
`mame.sqlite3`. MRA filename matching takes precedence over setname matching;
matched title, year, manufacturer, category, player, and control fields overlay
the MAME fallback. Category is persisted in the main catalog, navigation
snapshot, per-system shard, summary, and return capsule. Installing this build
over an existing catalog raises the generated schema versions, so the launcher
keeps the usable catalog visible while it builds and atomically publishes the
replacement; no in-place mutation of the old cache is attempted.

## Catalog Failure Reports

Catalog failures queue an atomic, bounded report under the active installation:

```text
diagnostics/catalog/latest.json
diagnostics/catalog/report-catalog-<timestamp>.json
```

`latest.json` uses schema `mister-magik-catalog-failure-v1`; reports are limited
to 64 KiB and the five newest timestamped reports are retained. They include
build identity, a stable failure code, operation and stage, schema or projection
expectations, affected system and generation, usable catalog counts, offered
recovery actions, and bounded catalog event history. Report-write failure never
blocks recovery. The volatile `status.json` also exposes the current structured
`catalog_failure`, and `events.jsonl` records stable failure and recovery events.

Catalog workers also persist a progress episode even when no operation returns
an error:

```text
diagnostics/catalog/progress-latest.json
diagnostics/catalog/progress-catalog-<timestamp>-<pid>-<sequence>.json
```

The episode records worker operation, execution mode and cooperative policy,
the current scan target and durable target/shard counters, the latest phase,
detail and percentage, activity counts, wall and active elapsed time,
catalog-state and resumable-build file metadata, projected runtime/Main
snapshots, and current-PID bounded catalog log tails. It is written at worker
start, at most every two minutes while running, after five minutes without
worker activity, on recovery from a stall, and on completion or failure.
Writes are asynchronous and bounded to 96 KiB each; the newest 24 episodes,
2 MiB, and 48 hours are hard retention limits.

For a sendable support bundle, run the typed host command:

```text
mister agent diagnostics --out PATH
```

Agent and SSH-fallback collection probe both public and development
installations. The bundle exports `catalog-failure-latest.json`,
`catalog-progress-latest.json`, and `latch-failure-latest.json` when those
reports exist. Collect it while a suspected stall is still active when
possible; the persistent progress episode remains available after restart.
A screen capture may clarify what the user saw, but is not required to identify
the catalog phase or last observed activity.

## Launch Return

Before handing a game to Main, the launcher writes a bounded return capsule for
the active collection and exact selection. On return, the capsule restores only
the rows and structured plans needed to reconstruct that view. It is bound to
the current catalog identity and rejected if stale or mismatched.

Return startup therefore does not open the complete catalog. If the capsule
cannot satisfy restoration, the launcher opens the registry and only the
selected system mini-nav. Preview readiness has a bounded hold and cannot leave
HDMI black indefinitely.

## Classification And Contents

Discovery associates each playable item with a stable system ID. The checked-in
`crates/catalog/data/system_taxonomy.json` then supplies the product platform
kind, launcher section/family, title, aliases, and order. Core location is not
classification authority: a console launcher stored under `_Arcade` remains a
console.

The scanner must not:

- walk screenshot or preview-cache media;
- read `gamelist.xml`;
- classify BIOS files, helper payloads, raw core binaries, or menu launchers as
  games;
- infer platform kind from filesystem placement when taxonomy exists.

Unknown systems remain visible under the taxonomy fallback and emit a
diagnostic. Invalid checked-in taxonomy or persisted platform-kind data is a
hard validation error.

## Standalone Testing And Inspection

The builder and projection machinery live in `crates/catalog` and run
without the UI:

```text
$magik-rust-lsp
git add -- PATH...
git commit -m "Describe the catalog change"
git push
scripts/agent benchmark
```

The analyzer supplies package-scoped Rust diagnostics during editing. Pre-push
and CI run the standalone suite and consumer assurance.
The standalone suite covers registry atomicity, shard integrity, lazy reads,
incremental reconciliation, scan checkpoints, first-visible bootstrap, and
the foreground-to-continuous-CPU0 transition.
Fresh-build reconciliation rows report `pipeline_overlap_us`,
`pipeline_queue_wait_us`, `pipeline_peak_in_flight`, and `pipeline_fallbacks`.
A qualifying pipelined run has positive overlap, never exceeds two in-flight
shards, and reports fallback only when tmpfs capacity forced sequential
on-media staging. `shard_build_wall_us` and `shard_publication_wall_us` report
the independently accumulated construction and publication wall time used to
interpret that overlap. `artifact_copy_hash_us` covers the fused copy/checksum
phase, and `artifact_publish_bytes` records bytes passed to the publisher.
The two first-scan invocations distinguish a genuine first-ever fallback from
the production retained-index recovery path; the latter removes `catalog-v3`
but deliberately preserves `arcade-bootstrap.nav.lz4b`.

Console, handheld, and computer projection groups discoveries by system and
canonical software-family identity. When no metadata identity exists, the
normalized base title before parenthetical or bracketed release annotations is
the family key. Multi-disc releases remain one family and retain their existing
launch contract. The representative is selected deterministically by supported
source, final/retail status, identity match, loose payload before archive
member, preview availability, stable version, and path. All discoveries and
launch plans remain in canonical catalog state; only the visible system shard
and navigation list are collapsed. Variants remain hidden until a dedicated
variant-selection UI is added.

Generic manifest profiles are authoritative for canonical system/core pairs.
An installed MGL descriptor may bind a system to a shared physical core only
when that core is declared in the profile's `compatible_core_names`. This
supports maintained shared implementations such as Atari 2600 through
`Atari7800` and Game Boy Color through `Gameboy` without turning matching
folders or extensions into core guesses. An exact canonical core takes
precedence when both are installed. Undeclared cross-system mappings are
omitted as unlaunchable.

ZIP members are stored as explicit archive-member launch references, never as
synthetic filesystem paths. Launch preparation validates the member path,
compression method, declared and expanded size, and checksum, then writes the
selected member to bounded `/tmp/mister-magik/launch-payloads` staging. Failed
preparation removes staging; the next launcher start removes payloads retained
for a successful Main handoff. Catalog storage is never used for extracted
launch material.

A launch failure restores framebuffer ownership and displays a persistent,
Back-only confirmation overlay containing the game title, concise failure copy,
and recovery guidance. A, B, or Home acknowledges it. Until then, the lifecycle
remains in launch-failure recovery and captures dialog input; the original
system, filter, scroll position, and selected row remain unchanged. Detailed
paths and error codes are logged but are not shown in the primary UI.

On a device, `mister-magik-fb catalog-v3-inspect` (or `scripts/agent device catalog inspect`)
eagerly verifies both manifest slots, artifact sizes and hashes, state binding,
scanner cache, every system shard, summed counts, duplicate visible family
keys, structured-plan payload readability, canonical system/core agreement,
and each system's keyed and available screenshot coverage. Per-system rows also report `source_games`,
`visible_families`, and `collapsed_variants`; shards written before these
optional metadata keys report their visible game count as the source/family
count and zero collapsed variants. This expensive command is acceptance
tooling, not a launcher startup path.

`scripts/agent benchmark` owns process/layout checks, deterministic cold data,
structured event evaluation, catalog contention, and unconditional restoration.
The scenario is selected from changed components and has no duration, label,
skip-build, or fixture flags.

## Deferred Alpha Qualification

Implementation and local validation do not publish or deploy an alpha. After a
separately authorized alpha publication, qualify the existing alpha-channel
device while preserving its schema-three, `rich-game-v2` catalog:

1. Install the published alpha through the normal downloader path and confirm
   that the format-upgrade dialog appears before rebuilding.
2. Choose Continue and launch a resident Arcade game.
3. Return to the launcher, choose the atomic rebuild, and verify schema-four,
   `rich-game-v2` artifacts and a newly published durable generation.
4. Reboot and verify that the rebuilt catalog remains authoritative and games
   still launch.
5. Collect `mister agent diagnostics --out PATH` and confirm the bundle contains
   `catalog-progress-latest.json`, plus `catalog-failure-latest.json` or
   `latch-failure-latest.json` when those failures were recorded.

This qualification does not require reset-fault testing and does not change the
device downloader INI or release channel.

## Ownership

- `catalog_config.rs`: roots and environment overrides.
- `catalog_state.rs`: schema-one stamp/checkpoint state.
- `scanner_cache.rs`: schema-one scanner cache.
- `shard_registry.rs`: schema-one manifests and artifact validation.
- `system_shard.rs`: schema-four per-system SQLite and schema-two mini-nav artifacts.
- `production_sharded_projection.rs`: production reconciliation, binding, and
  publication.
- `lazy_sharded_reader.rs`: registry-first and per-system reads.
- `builder_service.rs`: standalone builder lifecycle and first-visible boundary.
- `cooperative_work.rs`: UI-independent background checkpoints.
- `catalog_acceptance.rs`: full read-only V3 integrity report.
- `apps/mister/src/ui_runner/catalog_worker.rs`: launcher worker adapter.
- `apps/mister/src/ui_runner/launcher_scheduler.rs`: UI-side job scheduling.
- `apps/mister/src/return_catalog_capsule.rs`: bounded launch-return persistence.
