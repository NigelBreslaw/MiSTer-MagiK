# Fast Catalog

MiSTer MagiK uses one production catalog: the snapshot-driven fast catalog
stored under `/media/fat/mister-magik/catalog-fast-v1` (or the matching
development installation). The retired monolithic builder, scanner cache,
summary projection, navigation database, SQLite catalog, and builder sidecars
are not startup or refresh inputs.

The catalog has three independent responsibilities:

- source adapters discover playable games;
- immutable per-system artifacts provide navigation and search;
- source snapshots make later refreshes proportional to changed systems.

The UI never scans ROMs and never opens every system database at startup.

## System discovery

The adapter registry is dynamic. Every installed system recognized by the
launch-profile table is eligible; there is no fixed nine-system allow-list.
System identifiers, display titles, roots, core identities, accepted file
extensions, archive rules, and launch plans come from production profiles.

Five collections have prepared adapters because their upstream shape is known:

- Arcade combines Update_All metadata with installed MRA, core, and ROM
  validation. It also discovers supplemental MRAs and cores, including beta
  content, but an external-ROM MRA is never published without its required ROM.
- Amiga uses AmigaVision metadata and falls back to installed custom content.
- DOS uses the complete 0MHz inventory and falls back to installed custom
  content.
- X68000 uses Neon68K metadata and falls back to installed custom content.
- C64 uses OneLoad64 and folds language/title variants into launchable families.

All other systems use the generic profile-backed adapter. It walks the user's
installed roots and archive central directories. It does not contain a snapshot
of any particular MiSTer card. Hidden files, AppleDouble files, metadata
directories, partial downloads, and unsupported extensions are rejected before
rows are created.

Prepared and generic rows share the same canonical validation, stable-key,
sorting, deduplication, screenshot identity, and structured launch-plan rules.

## Published artifacts

The active manifest points to one immutable generation. Each system has:

- a NavPack containing the count, first-page rows, screenshot identity, compact
  navigation rows, and structured launch plans used for instant system entry;
- a search-only SQLite database containing the system's canonical searchable
  rows and FTS index.

The FTS index remains enabled and optimized in production. The builder accepts
bounded diagnostic arms (`MISTER_CATALOG_SEARCH_PIPELINE_BATCH=128|256|512|1024`)
and an opt-in `MISTER_CATALOG_SEARCH_DETAIL=column` comparison; neither arm
changes the default full-detail search behavior.

SQLite is not used to populate the first screen. NavPack is the immediate UI
database; SQLite is opened on demand for search. The registry contains system
titles, counts, platform kinds, artifact paths, sizes, and checksums.

Artifacts are written to generation-private paths. Publication writes and
syncs changed artifacts before atomically replacing the manifest. Unchanged
systems retain their immutable artifact references. Failed changed systems
retain their previous published artifacts; a newly discovered system that
cannot be built is omitted.

`mame.sqlite3` and `hbmame.sqlite3` remain CI/private-build source metadata for
Arcade classification and family knowledge. They are not runtime inputs,
launcher databases, or published distribution files; the runtime consumes the
compact `magik-metadata-v1.bin` container instead.

## Fresh build

The typed host command scripts/agent device catalog metadata-qualification --out
<evidence.json> validates the compact container with the v2
mister-magik-runtime-metadata-qualification-v2 report. Full device
acceptance is recorded only after the compact integrity gates pass and all
four forbidden legacy paths are absent:

    /media/fat/mister-magik/mame.sqlite3
    /media/fat/mister-magik/hbmame.sqlite3
    /media/fat/mister-magik-dev/mame.sqlite3
    /media/fat/mister-magik-dev/hbmame.sqlite3

The evidence records each path's presence state. A present path fails the
qualification; the host never opens these SQLite files.

A fresh build:

1. discovers installed systems from current roots and profiles;
2. inventories each generic root once, resolving its profile and creating rows,
   archive identities, directory fingerprints, and refresh watches from that
   retained traversal;
3. invokes prepared adapters for known collections, with OneLoad64 likewise
   deriving rows and refresh watches from one namespace inventory;
4. validates launchability and canonicalizes rows;
5. writes one NavPack and one search SQLite database per system;
6. publishes the active catalog manifest;
7. captures any remaining fallback watch indexes and all row snapshots for
   later refreshes.

Profile resolution never opens a generic root a second time. Unknown runtime
roots are inspected only to depth two; traversal resumes from the retained
frontier only after the root resolves to a launchable profile. Relevant ZIP
central directories are opened once after profile resolution. Arbitrary ROM
collections remain dynamically discovered—there are no card-specific generic
rows or filesystem snapshots.

All exFAT enumeration and archive access is serial. Artifact construction may
use CPU helpers internally, but publication never adds a second SD-card I/O
lane. Production counters report directory opens, ZIP opens, shallow and
resumed traversal, classification, row creation, refresh-watch reuse, SQLite,
NavPack, copy/hash, and publication timings.

The fresh path never reads old catalog artifacts. Removing
`catalog-fast-v1` and rebooting therefore exercises authoritative cold source
discovery. Production diagnostics report source, publication, snapshot,
per-system, byte, and row timings.

The generic source phase emits a bounded phase record separating
known-profile roots, runtime inventory, runtime profile resolution, resumed
continuation walks, finalization, and residual envelope time. Streamline
analysis can be given symbol images through `MISTER_STREAMLINE_SEARCH_IMAGES`
(a host path-list) so device samples resolve to application functions instead
of anonymous PCs.

## Incremental refresh

Refresh state is under `catalog-fast-v1/fast-refresh-v1` and is bound to:

- the active catalog generation and registry fingerprint;
- source-adapter and launch-profile compatibility identities;
- installed core identities;
- watched directories and archive/container identities;
- canonical row fingerprints.

The small watch index is separate from the larger cached row snapshot. A
no-change refresh stats known directories, parent anchors, and containers
without decoding cached rows or opening published NavPack/SQLite artifacts.
It performs no writes.

When a source unit changes, only that system's row snapshot is decoded. Deleted
or replaced source-owned rows are removed, affected directories or ZIP central
directories are rescanned, and the rows are merged, sorted, deduplicated, and
validated. Direct ROM contents are not hashed because the catalog identity and
launch plan are path-based.

If source facts changed but canonical rows did not, only refresh state is
updated. If rows changed, only that system's NavPack and search database are
republished. A missing, corrupt, incompatible, or generation-mismatched
snapshot causes a conservative system-level rescan.

Snapshot manifests use two atomic slots with fixed binary headers, bounded
decoding, checksums, and manifest-last publication. A crash can cause extra
work on the next refresh but cannot make stale snapshot state authoritative.

## Launcher lifecycle

Warm launch is load-only:

1. a launch-return capsule may restore the exact previous destination;
2. otherwise the active registry provides system counts immediately;
3. entering a system opens that system's NavPack on demand;
4. search opens that system's SQLite database on demand.

If no usable registry exists, the launcher keeps its first frame responsive and
starts a fresh fast-catalog build. Explicit Library Refresh runs incremental
update; an explicit rebuild runs the fresh path. Progress advances exactly once
per checked system as `Updating systems X/N`, where `N` is the dynamic union
of published and currently recognized systems.

When startup detects the retired `catalog-v3` or its top-level SQLite catalog
sidecars, it selects the cold particle-intro route, removes only those generated
predecessor artifacts, and runs a fresh fast-catalog build. Screenshot assets,
user state, game-database metadata, and installed games are preserved. If no
fast registry exists, startup selects that fresh build before deciding whether
the particle intro can run; portrait, benchmark, screensaver, return-from-game,
and unavailable-intro paths therefore never attempt manifest reconciliation.

The existing generation remains usable while a refresh runs. The launcher
reloads the registry only after successful manifest publication. Returning from
a launched core therefore restores into the same catalog architecture rather
than another builder's artifacts.

## Screenshot packs

Screenshot packs are media, not catalog authority. Rows store a deterministic
preview asset identity even when a pack is absent.

- The current screenshot manifest is downloaded whenever network access becomes
  available.
- Arcade's pack is requested regardless of whether Arcade is discovered.
- Other packs are requested only when the user enters that system's preview or
  game-list route.
- Manifest identity prevents downloading a current pack again.
- Network-unavailable retries are bounded and rescheduled until connectivity
  succeeds.

After a pack changes, preview availability is reconciled against the selected
system's NavPack in memory. Catalog artifacts and source snapshots are not
rewritten merely because media availability changed.

When a pack is installed or confirmed current, the media worker also performs
an in-memory MAME software-list title/family reconciliation for catalog rows
whose preview key is blank. It opens the numbered-release
`magik-metadata-v1.bin` header/index and reads only the selected system shard,
then intersects unique normalized title matches with the installed pack index.
Existing keys remain authoritative; ambiguous or missing matches remain blank.
This pass is non-fatal and never changes the catalog generation, manifest,
pack, or index formats. Runtime never opens the legacy SQLite metadata
databases; those files are CI/private-build inputs only. Use
`scripts/agent device catalog screenshots --system <id> --out <path>` to audit
the effective runtime rows without downloading or writing catalog state.

## Fault and performance rules

- Filesystem I/O is serialized on the exFAT SD card. CPU-only preparation may
  overlap only when measurement demonstrates a wall-time improvement.
- Hidden inputs are rejected during enumeration, before metadata extraction or
  row allocation.
- Uncertain I/O failure is not interpreted as deletion.
- Refresh parity is measured against a reboot-cold scratch build.
- Authoritative hardware timing uses two reboot-cold samples unless a narrower
  operator request explicitly accepts one.
- Production profiling reports snapshot checks, directory enumeration, ZIP
  reads, row reconciliation, SQLite, NavPack, copying/hashing, sync, publication,
  memory, and available Cortex-A9 counters.

## Relevant implementation

- `fast_catalog_sources.rs`: dynamic source adapters and prepared collections.
- `generic_system_catalog.rs`: profile-backed discovery for arbitrary systems.
- `fast_catalog_refresh.rs`: watch snapshots, planning, reconciliation, and
  incremental publication.
- `fast_five_catalog.rs`: canonical snapshot and artifact interchange. The
  historical filename is internal; it no longer imposes a five-system limit.
- `shard_registry.rs`, `system_shard.rs`, `nav_pack.rs`: immutable registry
  and per-system artifacts.
- `lazy_sharded_reader.rs`, `persisted_search.rs`: on-demand UI reads.
- `preview_availability.rs`: in-memory screenshot availability reconciliation.
- `apps/mister/src/ui_runner/catalog_worker.rs`: launcher integration.
