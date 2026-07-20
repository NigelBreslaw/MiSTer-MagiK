# Catalog V3

Catalog V3 is the only production catalog used by MiSTer MagiK. Its public
registry, navigation, state, binding, and scanner-cache schemas are version
**1**; the SQLite shard schema is version **2**. There is no legacy read
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
- Keep every background scan and rebuild phase subordinate to UI responsiveness.
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
  catalog.binding.json
```

The two manifest slots provide an atomic registry commit. Each manifest names
the active and, where retained, previous immutable SQLite/navigation pair for
every system, including sizes, hashes, metadata, generation, and game count.
Readers choose the newest completely valid slot. Partially written generations
are unreachable.

`catalog.binding.json` binds the active registry generation and the
`rich-game-v1` projection contract to the canonical fingerprint in
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

The retired files `library.sqlite3`, `library.summary.json`, and
`library.nav.lz4b` are not production inputs or outputs. Acceptance fails if a
current build recreates them.

## Startup And Lazy Loading

Warm startup reads the registry first. This supplies system titles, placement,
ordering, counts, and immutable artifact references without opening every
system database. Arcade's mini-nav is opened eagerly because it is the first
visible game collection. Other system shards stay closed until selected.

Selecting an unloaded system schedules one bounded background mini-nav load.
The loaded rows and structured launch plans are merged into the live catalog;
already loaded systems remain resident. A failed shard load is surfaced once
and does not retry every frame.

The following numbers are deliberately distinct:

- total registered games: sum of all active system counts in the registry;
- registered systems: active system entries in the registry;
- resident Arcade games: eagerly loaded Arcade rows;
- selected-system games: rows loaded lazily for the current non-Arcade system.

Resident Arcade rows must never be reported as the total catalog.

## First Build And Progressive Reveal

With no valid registry, the launcher shows its Slint splash immediately. It
first probes the bounded `arcade-bootstrap.nav.lz4b` index beside `catalog-v3`.
The index is an exact, checksummed Arcade mini-nav plus its source stamp. A
matching index restores the canonical Arcade rows and structured launch plans
without walking `_Arcade` or opening every MRA. Missing, corrupt, oversized, or
stale indexes fall back to the existing foreground Arcade scan. When that
bounded projection is acknowledged, the launcher can reveal Home and Arcade.
System discoveries update scanning tiles while the complete authoritative scan
continues.

The retained index is generated locally from each MiSTer's canonical catalog;
it is not a catalog of one developer's paths and is not assumed to match every
Update All installation. Different game sets and files therefore receive their
own exact index. The standard `/media/fat/_Arcade` root remains the production
Arcade bootstrap input, matching the existing foreground scanner.

After the first-visible boundary, the build becomes background work. The full
catalog is projected into per-system immutable artifacts and committed through
the registry. Only the final complete generation is catalog authority. The
bootstrap index is a disposable startup accelerator: it is published atomically
after a valid first-visible snapshot, refreshed from the completed full catalog,
and never used to suppress the authoritative background scan. That scan audits
weak filesystem stamps and replaces any initially stale projection during the
same launch.

Foreground first-visible Arcade work does not obey the background idle latch.
It must finish promptly because there is no usable game UI yet. It must also
remain free of preview-cache rebuilds and screenshot/media walks.

## UI-Cooperative Background Work

The launcher owns an idle latch. Once any catalog is usable, all heavy catalog
work uses lightweight cooperative checkpoints and waits while that latch is
closed. Checkpoints cover:

- builder and scan-plan boundaries;
- library and prepared-payload filesystem walks;
- candidate classification batches;
- archive inspection;
- collection and coverage processing;
- deferred projection and publication preparation.

Checks occur at bounded batches rather than by polling an atomic for every
file. A wait must not hold a filesystem iterator borrow, database transaction,
publication lock, or UI lock, and it must not emit progress that suggests work
advanced while paused. Reopening the latch resumes the same operation without
discarding discoveries or restarting the scan.

That latch is in-process pause/resume only. A first build also keeps disposable
durable progress in `catalog-v3/state/build-progress.sqlite3`. Completed scan
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
rebuilds retain the bounded producer/publisher pipeline.

Neither the latch nor the progress journal is catalog authority. Readers accept
only the normal publication chain: all immutable shards, artifact barrier,
complete manifest, catalog binding, scanner cache, and catalog state. The
journal is removed last after that chain succeeds; missing, corrupt, or
contract-mismatched recovery state is discarded and rebuilt.

The catalog crate owns only a generic cooperative-work permission signal and
background scope. It does not depend on Slint or launcher types. The UI runner
adapts the latch into that signal.

## Incremental Rebuild And Publication

The scanner computes the canonical catalog stamp and uses its scanner cache to
avoid unnecessary source work. Projection reconciliation compares each
candidate system with its currently published shard. Unchanged systems retain
their immutable artifact paths and generations; changed systems receive new
SQLite/navigation pairs; removed systems disappear from the next registry.

If no system changes, no new registry generation is published. If one system
changes, only that system projection is rebuilt even though the registry still
describes the complete catalog. The work ratio and measured elapsed speedup are
reported separately. A 10x result is a useful target and historical comparison,
not a release blocker; selection correctness and UI responsiveness are gates.

Publication is manifest-last and failure-atomic. Garbage collection may remove
only artifacts not referenced by the active or retained previous generation.

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

```bash
cargo test --manifest-path crates/catalog/Cargo.toml --features builder
cargo run --release --manifest-path crates/catalog/Cargo.toml \
  --features builder --bin catalog-lab -- rebuild-bench STORAGE SYSTEMS GAMES
scripts/bench-catalog-rebuild.sh LABEL
scripts/profile-first-scan.sh LABEL --drop-arcade-bootstrap-index
scripts/profile-first-scan.sh LABEL
```

The standalone suite covers registry atomicity, shard integrity, lazy reads,
incremental reconciliation, scan checkpoints, first-visible bootstrap, and
pause/resume behavior.
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
An installed MGL descriptor may provide a documented shared-core alias, but it
cannot advertise a known system through a different system's core. If the
canonical core is unavailable, that system is omitted as unlaunchable instead
of receiving a guessed core path.

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

On a device, `mister-magik-fb catalog-v3-inspect` (or `scripts/mister catalog`)
eagerly verifies both manifest slots, artifact sizes and hashes, state binding,
scanner cache, every system shard, summed counts, duplicate visible family
keys, structured-plan payload readability, canonical system/core agreement,
and each system's keyed and available screenshot coverage. Per-system rows also report `source_games`,
`visible_families`, and `collapsed_variants`; shards written before these
optional metadata keys report their visible game count as the source/family
count and zero collapsed variants. This expensive command is acceptance
tooling, not a launcher startup path.

`scripts/device-catalog-acceptance.sh` adds process/layout checks and ensures
legacy catalog artifacts are absent. `scripts/profile-catalog-contention.sh`
runs the 120-second `human-turbo-hold` overlap gate. A valid contention result
requires at least 600 frames overlapping catalog CPU work, at least 10 active
catalog intervals, zero work-budget violations, zero two-frame wall stalls,
zero vsync misses, and zero present failures.

## Ownership

- `catalog_config.rs`: roots and environment overrides.
- `catalog_state.rs`: schema-one stamp/checkpoint state.
- `scanner_cache.rs`: schema-one scanner cache.
- `shard_registry.rs`: schema-one manifests and artifact validation.
- `system_shard.rs`: schema-two per-system SQLite and schema-one mini-nav artifacts.
- `production_sharded_projection.rs`: production reconciliation, binding, and
  publication.
- `lazy_sharded_reader.rs`: registry-first and per-system reads.
- `builder_service.rs`: standalone builder lifecycle and first-visible boundary.
- `cooperative_work.rs`: UI-independent background checkpoints.
- `catalog_acceptance.rs`: full read-only V3 integrity report.
- `apps/mister/src/ui_runner/catalog_worker.rs`: launcher worker adapter.
- `apps/mister/src/ui_runner/launcher_scheduler.rs`: UI-side job scheduling.
- `apps/mister/src/return_catalog_capsule.rs`: bounded launch-return persistence.
