# Catalog V2

This document is the current contract for the MiSTer MagiK catalog system. It is
written for future UI and launcher work, so it focuses on lifecycle, public read
APIs, progress states, and benchmark expectations.

## Goals

- Warm boot with a usable catalog should show the first usable UI within 3.5s.
- Warm unchanged validation should be a root stamp check: under 500ms is the
  soft target, under 2s is the hard gate.
- Fresh catalog creation and explicit refresh both use the same full builder.
  Reference-MiSTer content acceptance is keyed by the versioned catalog stamp
  fingerprint in `scripts/catalog-fixture-contract.json`. The contract names
  the compatibility `games` view, physical `game_rows`, launcher-visible rows,
  and systems separately. Historical timing and database-size budgets are
  recorded by default and become gates only with an explicit performance flag.
- First database creation is a foreground bootstrap job. The catalog worker and
  library walker must run at full priority with unrestricted CPU affinity until
  the RAM catalog is usable. It is acceptable for the scan/build screen to
  reduce UI smoothness or drop frames during this window; meeting the first-scan
  readiness target is more important than preserving perfect animation while no
  usable catalog exists.
- Unchanged virtual launch cache materialization should complete under 2s and
  must not read every generated `.mgl` file.

## Files And Owners

- `magik-gui/catalog/src/catalog_config.rs` owns default roots, DB paths, schema
  version, and catalog build version.
- `magik-gui/catalog/data/system_taxonomy.json` is the checked-in, versioned
  product taxonomy for systems. `catalog_classify.rs` is its only parser and
  resolver. It owns stable system IDs, display titles, platform kinds, launcher
  sections/families, aliases, and ordering.
- `magik-gui/catalog/src/catalog_stamp.rs` owns the warm validation stamp.
- `magik-gui/catalog/src/catalog_store.rs` owns stamp persistence helpers.
- `magik-gui/catalog/src/catalog_build_record.rs` owns the completed-build
  duration sidecar used by the Info screen.
- `magik-gui/catalog/src/library_db.rs` remains the compatibility facade for
  scanning, classification, SQLite build/publish, and public read APIs while the
  catalog modules continue to split out.
- `magik-gui/src/ui_runner/catalog_worker.rs` owns catalog worker execution and
  progress messages.
- `magik-gui/src/ui_runner/launcher_lifecycle.rs` owns the launcher catalog
  readiness state (`SummaryProjection`, `FullSqlite`, or `FreshBuild`) and the
  transition from validation back to idle.
- `magik-gui/src/ui_runner/launcher_scheduler.rs` is the single launcher-side
  adapter for starting and polling catalog, media, launch, and background jobs.
- `magik-gui/src/launcher.rs` owns the library rebuild-on-next-boot marker plus
  virtual launch cache stamping and materialization.

## System Classification Authority

System association and system classification are separate decisions. Discovery
first associates a playable item with a stable system ID. The canonical system
taxonomy then resolves that ID to exactly one platform kind and launcher
placement. Launch profiles, discovery ordering, payload extensions, core names,
and filesystem locations are never product-taxonomy authorities.

In particular, `_Arcade/cores` is a launch-mechanics location. A core being
installed there does not make its system Arcade. The canonical classifications
include `sms` as Console / Sega, `gamegear` as Handheld / Sega, and `astrocade`
as Console / Other. A disagreement between core location and the canonical
result is retained in `system_classification_diagnostics`; it cannot override
the result.

Checked-in taxonomy parse failures, duplicate IDs or aliases, and missing
definitions for checked-in launch profiles are release-blocking validation
errors. Invalid persisted platform-kind values are fatal hydration errors and
must reach the visible library-load failure flow rather than silently becoming
`Unknown` or publishing a ready catalog. A genuinely new runtime system remains
visible through the explicit `Unknown` classification and is placed under
Consoles / Other with an audit diagnostic until its taxonomy row is added.

Schema 65 persists `systems.platform_kind` and
`systems.classification_source`, with a constrained set of normalized platform
kind values. RAM construction, SQLite hydration, summary projection,
navigation projection, and launcher hierarchy all consume that resolved value;
none re-infers classification from a core path.

The root Arcade tile is a virtual collection derived only from taxonomy rows in
the Arcade launcher section. Its displayed number is exactly the length of its
visible, preferred, parent-collapsed game projection. Alternatives, bootlegs,
clones, Neo Geo, SMS, Game Gear, Astrocade, and every console/handheld/computer
system are excluded from that number. There is no fallback to raw system sums
or broader physical `_Arcade` membership.

## Read-Only Inspection

Use `scripts/mister catalog` for routine counts and launch lookups. Use
`scripts/mister db`, `scripts/mister library-db`, or
`mister-magik-fb library-sql` for catalog database inspection. `library-sql` is
available in release-device builds so device queries do not need `sqlite3(1)` or
a diagnostics binary. These entrypoints open SQLite read-only/query-only,
prepare exactly one statement, and use SQLite's read-only classification plus a
safe introspection-PRAGMA allowlist. Read-only `PRAGMA table_info(...)` and
`EXPLAIN QUERY PLAN` are supported.
Do not add write, repair, migration, or cache rebuild behavior to these
inspection paths; use the catalog builder and launcher worker flows instead.

`library-sql` prints normal query rows first, then appends one
`library_sql_timing_tsv` row with path, database size, query hash, open/prepare,
first-row, row-read, formatting, total elapsed time, row count, column count, and
query-output byte count. If `scripts/mister db` reports that it is using the
SFTP fallback, the timing describes the host-side local query of a copied
database, not direct device SQLite performance.

Pass repeated `--query SQL` arguments to reuse one SSH command, remote process,
and SQLite connection. Batch output wraps each result with
`library_sql_result_tsv` rows and ends with `library_sql_batch_tsv`. Routine
scripts should query physical tables such as `game_rows`, `launch_target_rows`,
and `launcher_catalog_rows`; text compatibility views such as `games` and
`launch_plans` may decompress path data and are intended for human inspection.

## Launcher Search

The Arcade `Search` filter is a runtime UI index, not a catalog table or a
SQLite hot path. The background catalog worker builds normalized search keys
and a small autocomplete word index before delivering the full catalog, so the
first non-empty query performs no one-time construction on the UI thread. The
index uses each
game's title, launch path basename, manufacturer, category, year, and decade.
Typing on the virtual keyboard scans only the active system's in-memory rows and
updates the Rust-painted result list plus the Slint suggestion strip in the next
UI sync.

Search matching treats punctuation as whitespace and ranks visible fields above
launch-path matches. Metadata is first-class: a query such as `capcom` matches
games whose manufacturer is Capcom even if the title does not contain that word.
Autocomplete remains separate from result search; it suggests one word for the
current partial token, prioritizing current-system title and metadata words over
noisy path or region tokens.

Real-library search expectations can be checked with a local, ignored fixture:

```bash
mkdir -p private/test-fixtures
scripts/mister db "SELECT system_id,title,launch_ref,COALESCE(year,''),COALESCE(manufacturer,''),COALESCE(category,'') FROM launcher_catalog ORDER BY system_id,title" > private/test-fixtures/autocomplete-launcher-catalog.tsv
cargo test --manifest-path magik-gui/catalog/Cargo.toml arcade_search
```

## Lifecycle

Cold or reset database:

1. Launcher starts immediately and presents Slint UI.
2. Catalog worker treats missing, empty, or old-schema DBs as unusable.
3. Worker runs `ForceBuild`.
4. Full-screen scan UI is visible while the fresh catalog is scanned and
   projected.
5. The builder scans source game locations under `/media/fat` and keeps scan
   facts in Rust memory.
   This first-build scan and RAM catalog projection run foreground: they should
   not inherit the low-priority CPU0 background policy used by warm validation,
   media, or preview prefetch work. The UI may drop frames on the scan screen
   while this foreground builder consumes both Cortex-A9 cores.
   After the namespace scan joins, the foreground coordinator builds the RAM
   catalog while one scoped `catalog-audit` worker computes the exact deferred
   coverage audit and stamp from the same immutable scan. Both branches use the
   `CatalogForeground` nice-0/all-online policy and must join before the stamped
   navigation snapshot or `Ready` event can escape. The worker does not outlive
   the scan artifact and does not change audit, stamp, catalog ordering, or
   recovery semantics.
6. The worker reports `Ready` from the fresh RAM catalog as soon as it can
   provide a usable launcher catalog. The launcher clears the scan UI at this
   point and records `library_ready`.
7. The worker continues durable persistence in the background, creates SQLite
   under `/tmp/mister-magik/sqlite-build` for production `/media/fat` databases,
   embeds the stamped canonical navigation projection in that database,
   publishes the completed file, then reports `Persisted` and records
   `library_db_saved`. Before reporting success, the catalog builder atomically
   writes `/media/fat/mister-magik/database-build-time.txt` with total build
   duration rounded to the nearest second. Both UI-triggered and standalone
   builder runs use this path; the Info screen only reads and formats it.
8. Virtual launch cache materialization runs after readiness so it cannot extend
   first usable catalog time.

The finalized RAM catalog owns preferred variant collapse, platform kinds,
structured launch plans, preview identity, and final list ordering. The
production writer stores its exact stamped navigation projection inside SQLite
in the same transaction as the source facts. After the database is published,
the adjacent warm-start projections
(`library.summary.json` and `library.nav.lz4b`) are written from the same
finalized catalog. If publication is interrupted between those steps, the
embedded projection is the exact recovery source for recreating the pair.

Schema 65 temporarily retains populated `ui_arcade_*`,
`launcher_catalog_rows`, and related materialized compatibility tables. Release
acceptance, diagnostics, and benchmark selectors still query them while they
are migrated to the canonical navigation contract. These tables are not the
runtime source of truth. The production canonical writer populates them from
the same RAM catalog generation and retained Arcade discovery identities; it
does not rebuild them by traversing the path-decompressing text views.
Compatibility launch identity is the tuple `(launch_ref,title,system_id)` so
distinct collection games may safely share one launch reference. Publication
fails on any scan/catalog identity mismatch. The legacy SQL materializer remains
for legacy/test writer coverage only. These tables may be removed only after
their selector migration lands with equivalent device coverage.

Legacy/source-fact databases may still be readable through the joined-SQL
fallback, but that result is explicitly degraded: preview keys and finalized
ordering/variant semantics are not guaranteed. The launcher may use it to stay
available, but must never publish it as a replacement summary/navigation pair.

Compressed catalog projections, including the SQLite-embedded navigation blob,
are untrusted stored input: read their LZ4 decoded length before allocating,
reject it above the owning format's bound, then decode into exactly that buffer.
A malformed, stale, or oversized projection is treated as a failed warm cache
and must not make the launcher allocate an unbounded buffer.

The Settings-screen `Reset Database` action removes the SQLite catalog, its
adjacent `library.summary.json` and `library.nav.lz4b` projections, the
`database-build-time.txt` duration record, and all recognized screenshot pack
files under `/media/fat/mister-magik/assets` before requesting the supervised
reboot. It deletes size-qualified and legacy `<system>-screenshots*.mmlz4b`
files for supported pack systems plus `.screenshot-media-state.json`; unrelated
files in the assets directory are left alone.

Warm boot with a usable cache:

1. Launcher may load `library.summary.json` as a `SummaryProjection` so Home and
   system counts are usable immediately.
2. A current stamped `library.nav.lz4b` is the preferred full navigation path.
   If it is absent or invalid, the worker opens SQLite and first attempts the
   embedded canonical navigation payload before retained materialized
   compatibility recovery. A corrupt embedded payload falls through without
   making an otherwise readable database unusable.
3. After the first visible copy and configured warm-validation delay, the
   scheduler starts the catalog worker. Summary-only boots use `ProbeSqlite` so
   full navigation is hydrated; boots that already loaded full navigation use
   `AlreadyLoadedReady`.
4. A matching current-schema SQLite catalog transitions through `FullSqlite`
   readiness without forcing a rebuild.
5. Worker runs `CheckStamp`.
   Before starting a deferred background check, the launcher probes the shared
   catalog-builder exclusion lock. If a standalone builder owns the lock, the
   check remains queued and retries after one second; no worker thread or
   subprocess is started. The UI remains active throughout standalone builds.
6. If the stored stamp matches the current root stamp, the worker reports
   `Unchanged` and does not rebuild.
7. If the stamp is missing, stale, or cannot be checked, the worker reports
   `Changed` and exits. It must not run the full builder automatically.
8. The launcher shows a `Library changed` confirmation dialog. `Continue` keeps
   the current catalog for this session and writes
   `/media/fat/mister-magik/rebuild-on-next-boot`. `Rebuild` immediately starts
   a foreground `ForceBuild`.
9. On the next MagiK boot, the launcher consumes the rebuild marker as a
   one-shot request and starts the foreground `Updating Library` flow instead of
   delayed ready-cache validation.

If existing catalog artifacts cannot be loaded, the launcher lifecycle enters
`CatalogLoadFailed` and shows `Library failed to load.` with the underlying
error plus `Retry` and `Rebuild`. `Retry` is a strict read of the current
artifacts and can never fall through to a build. `Rebuild` starts the dedicated
`fresh-build` operation. After taking the catalog-builder lock, that operation
removes the SQLite database, summary/navigation projections, build-duration
record, recognized catalog temp files, stale ready snapshots, and rebuild
marker before scanning from source. Screenshot packs and media state are not
catalog artifacts and are preserved. A genuinely missing or empty first-boot
catalog remains the normal automatic first-build path.

The summary projection contains systems, normalized platform kinds, and
counts, not per-game rows. Home builds the same pruned manufacturer hierarchy
from it immediately; full Arcade rows included in the hot seed also provide
the `SNK` manufacturer shortcut count. A user may enter a collection from
those tiles, but the game list must treat a selected non-hydrated collection as
a loading state until the full SQLite
rows hydrate. During that state the Rust-painted game list and preview requests
stay paused, a Slint loading overlay covers the list viewport, and the overlay
fades away when the hydrated rows are first presented. Arcade benchmarks that
start or lock directly on Arcade must wait for hydrated rows before their timed
movement begins.

Explicit refresh and chosen rebuild:

1. UI, marker boot, or CLI requests `ForceBuild`.
2. The full builder always runs.
3. V1 drift detection does not partially mutate the SQLite catalog. It detects
   that a rebuild is needed, then reuses the full builder as the only write
   path.

## Discovery Checkpoint

Warm validation uses both the root stamp and a compact discovery checkpoint.
The checkpoint is persisted in SQLite next to the stamp and records only cheap
catalog inputs:

- schema, catalog build, profile set, and core launch manifest identity,
- configured roots and core search root metadata,
- installed core summaries from `_Console`, `_Computer`, `_Arcade/cores`, and
  `_LLAPI`,
- top-level game directory summaries under configured game roots,
- MAME and HBMAME metadata DB signatures,
- catalog coverage audit summary rows.

The checkpoint deliberately does not enumerate every game file. Normal copy and
update workflows that add systems, cores, or top-level game directories are
detected during delayed warm validation. Nested file-only changes still rely on
root metadata changing or an explicit refresh.

Checkpoint drift is advisory. `CheckStamp` reports `Changed` and exits when a
known core is added, removed, or has a changed file signature; when an unknown
core has a launchable-looking game directory; when a new top-level system game
directory appears; or when checkpoint storage is missing or stale. The launcher
then uses the existing `Library changed` dialog: `Continue` keeps the old
catalog for this session and writes `rebuild-on-next-boot`, while `Rebuild`
starts a foreground full build immediately.

Runtime discovery is hybrid. Baked Main-derived profiles still make known
systems fast, but foreground full builds also consider every top-level
`games/*` directory under the configured game roots. A top-level folder becomes
catalogable only when the installed cores and known launch facts produce a safe
runtime profile with playable payloads. Unknown folders, missing cores,
unsupported payload extensions, empty/media-only folders, and ambiguous aliases
remain `catalog_audit` diagnostics instead of becoming launcher systems.

Runtime system association is evidence-ranked. Top-level system `.mgl`
descriptors are authoritative: `<setname>` names the distinct platform and
`<rbf>` resolves its real backing core. Exact and punctuation-insensitive core
names rank next, followed by narrowly documented aliases; shared extensions are
only weak evidence and cannot override a descriptor or strong name match. This
keeps variants such as GBC, Mega Duck, Atari 2600, and WonderSwan Color separate
even though they share an RBF with another platform.

After a strong association, the scanner may inspect bounded ZIP central
directories to learn member extensions without decompressing payloads. Firmware
can support association but never becomes a game. Numbered boot ROMs, BIOS,
state/configuration files, tools, blank media, installers, support archives, and
demos are excluded by the generic content-role classifier. Runtime-inferred
systems require at least two remaining playable entries before appearing in
the launcher; suppressed files and folders remain queryable through
`catalog_audit`.

TSConf is intentionally support-only when its folder contains `SDCard.zip` or
`alt_roms.zip`. Its core mounts VHD virtual SD media; games inside those images
cannot become individual launcher rows until MagiK can both index the image and
select a deterministic internal title after boot.

## Discovery Dates

The SQLite catalog stores nullable per-game `discovered_at_unix` metadata.
Because production rebuilds create a fresh database and publish it atomically,
the builder reads the previous database before writing the replacement:

- a matching `game_id` keeps its previous discovery timestamp, including `NULL`
  baseline values from catalogs that predate discovery tracking,
- a game absent from the previous database receives the current scan timestamp,
- a missing or unreadable previous database is treated as the first baseline and
  writes `NULL` discovery timestamps for all rows.

The launcher derives the `New` list badge at load time from
`discovered_at_unix`; rows first seen within the last 14 days are new. The badge
state is not persisted separately. Materialized list tables carry
`discovered_at_unix` only so every list projection can derive the same state.

## Worker Requests

`CatalogWorkerRequest` has five modes:

- `LoadOnly`: use the already loaded cache and do no background validation.
- `StrictLoad`: read the current SQLite catalog only; report a load failure for
  missing, empty, or unreadable data and never build or delete anything.
- `CheckStamp`: validate the ready cache after the UI is usable.
- `ForceBuild`: run the full builder regardless of cache state.
- `FreshBuild`: acquire the builder lock, delete catalog-owned artifacts, and
  continue directly into a foreground scan/build.

Missing, empty, and old-schema caches are not usable. They always plan
`ForceBuild`, even when the request was `CheckStamp`, except that `StrictLoad`
always remains load-only and reports the failure. `FreshBuild` always uses its
distinct destructive plan. These requests map to the four internal worker plans
`LoadOnly`, `CheckStamp`, `ForceBuild`, and `FreshBuild`.

`MISTER_CATALOG_REFRESH=off` disables catalog validation and automatic builds
for a benchmark restart. If the summary is unavailable but an SQLite artifact
exists, the launcher still starts one deferred load-only worker after the first
visible copy: a valid cache hydrates the UI, while an unusable cache enters the
normal `CatalogLoadFailed` Retry/Rebuild flow. A genuinely missing cache stays
missing and is not built under this policy.

`MISTER_CATALOG_REFRESH` is the single launcher policy knob:

- unset: normal behavior; ready caches get delayed `CheckStamp` validation,
- `on`/`force`: force a full catalog rebuild,
- `off`/`load-only`: do not validate or build; use a summary immediately when
  available, otherwise hydrate an existing SQLite cache after the first visible
  copy.

## UI Progress Semantics

The full-screen scan UI is for foreground first-scan or chosen forced-build
states. Cold first scans keep the first-run `Scanning for games...` copy.
User-triggered and marker-triggered rebuilds use the `Updating Library` copy.

When a cold or reset database uses the staged RAM catalog path, the worker first
runs a cheap direct launcher bootstrap over top-level `_Arcade` `.mra` and
`.mgl` files. This is not a synthetic count: it only reports files that exist on
the device. The bootstrap exists to put a real game counter in motion before the
recursive metadata/classification pass has enough discoveries to report.

Cold first-scan game-count display is intentionally monotonic. The bootstrap
phase may establish an early target floor so the visible counter keeps moving
slowly while the full scanner catches up. Later `Classifying library` updates
must not pull the displayed number down; they take over only after their real
discovery count exceeds the currently displayed count.

After classification completes, the foreground screen switches from the game
counter to real RAM-catalog projection progress: preparation discovery count,
playable-row resolution, launcher-index construction, navigation snapshot
creation, and library opening. Counter details include an elapsed `still
working` heartbeat after one second; this confirms liveness without claiming an
ETA. SQLite persistence remains post-`Ready` background work.

The bottom-right `scanning...` badge is progress-driven background UI. It appears
only when the worker emits real background progress after a usable catalog is
already visible. It clears on `Unchanged`, `Ready`, `Done`, persistence failure,
scan failure, or load failure.

`CheckStamp` itself should normally be silent. A matching stamp produces timing
and `Unchanged`, not a visible progress badge. A stale or failed stamp check
emits `Changed`, which opens the `Library changed` dialog; it does not show the
first-run scan screen and does not turn the background badge into a rebuild.

If a foreground rebuild fails but the old SQLite catalog is still readable, the
launcher clears the update screen, shows a dismissible `Library update failed`
dialog, and continues with the old catalog. The failure remains in logs so the
user is not trapped away from the launcher.

Survivability is a catalog contract. Wild SD cards often contain unsupported
folders, BIOS payloads, cue support tracks, duplicate aliases, unknown raw ZIP
sets, and partially installed systems. Those inputs should reduce coverage, not
block boot. When MagiK can recover any launchable subset, it should publish or
keep a usable catalog and record skipped or suspicious inputs in
`catalog_audit` and logs. A true scan, save, or load failure must surface as a
visible, continuable error such as `Library scan failed`, `Library load failed`,
or `Library update failed`; it must not leave the launcher stuck indefinitely on
an indexing or saving progress screen.

## Root Stamp

The root stamp is the cheap unchanged validation. It includes:

- catalog schema version and catalog build version,
- launch profile set version,
- core launch manifest version and fingerprint,
- configured roots and root metadata,
- catalog coverage audit rows for installed cores and unindexed launch surfaces,
- MAME and HBMAME metadata DB signatures.

The stamp intentionally does not enumerate source-backed scan target
directories, nested game directories, or files. That keeps warm validation to a
small fixed set of metadata reads on cold SD-card caches. Normal copy/update
workflows are expected to update one of the configured root directories, such as
`/media/fat/games` or `/media/fat/_Arcade`. If a workflow changes only files or
directories below an unchanged root, explicit refresh remains the full rebuild
escape hatch.

## Game File Scan Scope

The full builder is not a media crawler. It scans game files only:

- launcher descriptors such as `.mra` and `.mgl`,
- core payloads and disk images from source-backed launch profiles, such as
  `.chd`, `.cue`, `.adf`, `.hdf`, `.vhd`, and cartridge ROM extensions,
- compressed containers when a profile supports launchable entries inside them,
  currently ZIP central-directory scanning for NeoGeo `.neo` entries,
- installed collection listing text files that belong to a game launcher.

Preview images, screenshot packs, manuals, box art, core `.rbf` files, and cache
media are not catalog inputs. `_Arcade/media` and `_Arcade/cores` are pruned
before recursive walking; game-bearing `_Arcade` trees such as `_alternatives`
remain in scope.

Main_MiSTer is the source for the directory model. `SelectFile` resolves normal
file pickers through `user_io_get_core_path(...)`, which maps to `games/<core>`
except for source-defined special suffixes such as PCE-CD and NeoGeo-CD. MagiK
models this with an active `ProfileSet`: explicit special profiles, generated
manifest rows for installed generic cores with source-backed extensions, and
runtime-discovered top-level profiles for folders that match installed cores
and known payload rules.

The scanner remains bounded. It checks special roots plus top-level `games/*`;
it does not crawl arbitrary SD-card trees to discover systems. For each
top-level game folder, the runtime planner can activate:

- exact case-insensitive folder/core matches,
- safe `*-Sinden` aliases when the base installed-core strategy is unique,
- unique extension-based aliases such as `games/Coleco/*.col` resolving to an
  installed `ColecoVision` core.

When several top-level folders resolve to the same runtime core, the active
profile list keeps a single profile ID and merges the distinct game directories
into it. For example, `games/Gameboy` and `games/Gameboy-Sinden` both map to
one `runtime-gameboy` profile row; SQLite profile IDs remain unique while both
folders stay catalogable.

It does not guess when multiple installed cores accept the same payload
extension, and it does not create launcher systems for folders with no playable
payloads. Folders that cannot be cataloged are recorded in `catalog_audit`
instead of being silently skipped.

Raw `games/mame/*.zip` and `games/hbmame/*.zip` folders are treated as Arcade
coverage inputs, not independent launcher rows. A zip set is visible only
through an existing launchable Arcade/MRA target for the same set; unknown raw
zip sets remain diagnostics and must not create dead virtual launch entries.

The default scan does not include `_Games`. That tree is treated as an organizer
mirror of generated `.mgl` launchers for games already available through
source/core game directories. Set `MISTER_LIBRARY_ROOTS` explicitly for a
diagnostic build that includes `_Games`.

## Supported Launch Profiles

Catalog profiles come from two sources:

- explicit special profiles for behavior that needs hand modeling: MRA, MGL,
  DOS installed MGL launchers, Saturn, PlayStation/PSX, AO486, Amiga/Minimig,
  AmigaVision, NeoGeo ZIP/XML behavior, NeoGeo CD disc images, and raw
  MAME/HBMAME zip-set coverage;
- generated generic profiles from
  `magik-gui/catalog/data/core_launch_manifest.json`, activated only when a
  matching `.rbf` is installed in `_Console`, `_Computer`, `_Arcade/cores`, or
  `_LLAPI`.

Generated manifest rows carry the core ID, title/category, expected
`games/<core>` directories, payload extensions, mount behavior, archive-entry
support, and source evidence. The checked-in manifest is the runtime source of
truth for known generic auto-cataloging. Runtime-discovered profiles extend that
set only from top-level game folders that have an installed core, known payload
extensions, and non-ambiguous launch semantics.

The manifest can be regenerated from host-side evidence gathered from the
maintained Main_MiSTer checkout and installed device cores. Observed core file
fingerprints are diagnostic confidence only; runtime activation remains based
on canonical core IDs and source-backed manifest rows so updated `.rbf` builds
do not lose catalog support.

Updating special profile semantics must bump `PROFILE_SET_VERSION`. Updating
the generated manifest must update the manifest version or fingerprint covered
by the catalog stamp. Unknown installed cores stay diagnostics until a
source-backed manifest row or special profile is added.

## Catalog Coverage Audit

Every full scan writes `catalog_audit` rows. Query them with
`scripts/mister db "SELECT * FROM catalog_audit ORDER BY catalog_status, expected_game_dir"`.

Useful summary queries:

```bash
scripts/mister db "SELECT system_id, count(*) FROM launcher_catalog GROUP BY system_id ORDER BY system_id"
scripts/mister db "SELECT catalog_status, source, reason, count(*) FROM catalog_audit GROUP BY catalog_status, source, reason ORDER BY count(*) DESC"
```

The audit records:

- installed `.rbf` cores in normal core locations that have no catalog profile,
- top-level `games/*` folders skipped by hybrid discovery, including
  `no-installed-core`, `no-valid-games`, `unsupported-extension`, and
  `ambiguous-alias`,
- runtime-discovered folders that were cataloged, with source
  `runtime-discovered`,
- collection ZIPs in loose-file-only profiles that will not be indexed.

Audit rows also expose `evidence_source`, `evidence_confidence`, `content_role`,
and `suppression_reason`, so an association or promotion decision can be
explained without reproducing the scan.

Audit rows are diagnostic only. They do not become launch rows until a real
profile is added. The audit is part of the catalog stamp, so new cores from
`update_all` can invalidate the warm catalog check. Rebuilds record coverage
diagnostics in the runtime event log and `catalog_audit` table without showing a
user-facing prompt.

## Collection Listings

Normal copied-in games, MRA/MGL launchers, installed AmigaVision HDF listings,
and generated launch plans are handled by the full builder.

Archive-embedded collection listings are opportunistic. Extracting those
listings shells out to `/media/fat/linux/7za`, so the default timeout is one
second per listing path. This preserves the fresh-build gate on slow exFAT media
while still allowing fast helpers to contribute extra collection rows. Set
`MISTER_7ZA_TIMEOUT_SECS` for diagnostics when measuring or debugging collection
listing extraction. 7z payload internals are not indexed as individual games
unless a profile has a source-backed listing reader for that archive.

AmigaVision is product-supported through the Amiga profile. Installed
`AmigaVision*.hdf` media and `AmigaVision*.7z` archives generate the stable
AmigaVision launcher row, and archive listings under
`games/Amiga/listings/games.txt` and `games/Amiga/listings/demos.txt` enumerate
individual AmigaVision titles. The archive itself is not a direct launch ref.

## Preview And Metadata Artifacts

Screenshot packs are fixed runtime artifacts, not catalog inputs. The runtime
loads LZ4-block `.mmlz4b` packs from `/media/fat/mister-magik/assets`; matching
`.mmlz4b.idx` sidecars are seek indexes for first-preview latency and the
source of truth for fast preview-availability refresh. The catalog stores only
the pack path, asset key, and current availability bit needed to request a
preview. Raw preview archive formats and ad hoc on-device pack generation are
retired. Build and publish packs from the private `private/magik-cloud`
submodule; this repo keeps only runtime preview loading, catalog projection,
and device acceptance checks. Use `scripts/magik-cloud path` to resolve
`MAGIK_CLOUD_DIR`, the submodule, or the legacy `../magik-cloud` checkout. See
`private/magik-cloud/docs/media-build.md` for the media-build workflow.

Remote screenshot-pack updates are manifest-driven. On-device MagiK and the
host commands `scripts/mister media-check` and `scripts/mister media-download`
read the Cloudflare R2 manifest from `MISTER_MEDIA_MANIFEST_URL`,
`--manifest-url`, or the default
`https://assets.mistermagik.com/mister-magik/v1/manifest.json`.

Runtime downloads save new packs with the image size in the filename:

```text
/media/fat/mister-magik/assets/arcade-screenshots-320x320.mmlz4b
/media/fat/mister-magik/assets/arcade-screenshots-320x320.mmlz4b.idx
/media/fat/mister-magik/assets/neogeo-screenshots-320x320.mmlz4b
/media/fat/mister-magik/assets/neogeo-screenshots-320x320.mmlz4b.idx
/media/fat/mister-magik/assets/saturn-screenshots-320x320.mmlz4b
/media/fat/mister-magik/assets/saturn-screenshots-320x320.mmlz4b.idx
```

Legacy catalog paths such as `arcade-screenshots.mmlz4b` remain valid lookup
keys. The preview worker resolves those legacy paths through the media state to
the preferred size-qualified pack and falls back to legacy fixed-name files when
needed. Current public packs are `320x320`; future smaller packs must preserve
their size in the local filename.

`mister-magik-fb preview-index-refresh-bench LABEL` refreshes preview
availability in the existing SQLite catalog from installed `.mmlz4b.idx`
sidecars. It does not rescan game directories or decode screenshot payloads.
For each supported screenshot-pack system, it resolves the active pack, reads
only the sidecar membership, and updates `has_preview` in `launcher_catalog`,
`ui_arcade_preferred`, and `ui_arcade_variants` for that system. Missing packs
or missing sidecars clear availability for that system and emit timing rows
instead of failing the command; malformed sidecars emit error rows and leave the
system untouched.

The downloader streams the raw pack object into `/tmp/mister-magik-media-download`
while feeding the same bytes to the SHA-256 verifier. This keeps the visible
download phase off MiSTer's slow exFAT/FUSE `/media/fat` filesystem while the
catalog scanner is also walking game directories. After verification, the
worker copies the staged file into a hidden temporary file beside the final pack
and publishes it atomically with file sync, rename, and parent-directory sync.
When the manifest includes an `index` block, it starts the small `.mmlz4b.idx`
sidecar download alongside the raw pack, keeps sidecar work out of visible
progress, and verifies the sidecar before marking the pack current. A local
pack is current only when the raw pack and its advertised sidecar match the
media state. If the raw pack is current but the sidecar is missing or stale, the
worker repairs only the sidecar without showing a progress row unless the
repair fails. The state file
`/media/fat/mister-magik/assets/.screenshot-media-state.json` records the last
successful media update, preferred size, index metadata, and the latest
HTTP/cache headers observed for each downloaded pack. It is not a catalog stamp
input.

During catalog scans, the scanner emits the first discovery of each supported
screenshot-pack system: `arcade`, `neogeo`, `nes`, `snes`, `n64`, `sms`,
`megadrive`, and `saturn`. The launcher starts the runtime media worker on the
first discovered supported system and queues only those discovered systems. If
the launcher starts from an already-usable SQLite catalog and no scan discovery
events will fire, it seeds the same selective requests from the catalog's
installed system list after the first rendered frame. Unrequested packs are not
checked or downloaded. The worker fetches the manifest once, de-duplicates
system requests, and runs one active pack download at a time. The active pack
may download its small index sidecar in parallel with the raw pack. The active
raw-pack download runs at normal priority and unrestricted CPU affinity so its
streaming thread, `curl`, and SHA-256 verifier are not starved by foreground
catalog creation. The media coordinator and index sidecar work stay in the
background scheduling class.

`MISTER_MEDIA_UPDATE=off` disables the media worker,
`MISTER_MEDIA_UPDATE=check` reports status without downloading, and
`MISTER_MEDIA_UPDATE=download` is the default. `MISTER_MEDIA_SIZE` defaults to
`320x320`. The runtime fetches the HTTPS manifest trust root, then downloads
verified pack and index objects over HTTP from the Cloudflare cached pack path.
Progress is emitted as structured `screenshot_media_progress` startup events
with system, size, phase, byte counts, pack index/count, and optional download
Mbps. The catalog-build screen renders compact active pack rows from the same
events. Each visible pack row uses one normalized progress bar: the `/tmp`
network download fills 0-100%, and the verify/save/sync/rename finalization
phases stay at 100%. The row omits byte labels and keeps completed packs
visible briefly so users can see every requested pack finish.

The production path is the canonical `.mmlz4b` object served with
`Accept-Encoding: identity`. Runtime uses manifest `compression: "none"` for the
raw pack plus optional `codec: "mmlz4b-index-v2"` sidecars. The preview loader
uses the sidecar for `index_pread` only while the full archive is not yet loaded;
steady state returns to `archive_mem`. Malformed sidecars, duplicate entries,
archive-size mismatches, and out-of-bounds entry ranges are treated as fast-lane
misses and fall back to the full archive path. MagiK does not decode or
benchmark gzip/Brotli screenshot objects on device.
Cloudflare compression behavior can still be inspected separately through
header probes:

```bash
scripts/mister media-cloudflare-check --system megadrive
scripts/profile-screenshot-download.sh LABEL --system megadrive --variant identity
```

`media-cloudflare-check` probes response headers with `Accept-Encoding:
identity`, `gzip`, and `br`. If `CLOUDFLARE_ZONE_ID` and
`CLOUDFLARE_API_TOKEN` are set, it also performs read-only Cloudflare API checks
for the zone Brotli setting and response-compression rules. Dashboard
verification is under **Speed > Optimization** and **Rules > Compression
Rules** for the R2 custom domain. Production should use an R2 custom domain, not
the development `r2.dev` endpoint, so Cloudflare Cache and compression rules are
available.

Runtime and benchmark logs record cache evidence from response headers:
`ETag`, `Last-Modified`, `Cache-Control`, `Age`, `CF-Cache-Status`, `CF-Ray`,
`Content-Length`, `Content-Encoding`, and effective URL. A missing or uncached
Cloudflare header is evidence for setup/debugging, not a download failure.

Screenshot download benchmark rows must include network download, the
decompression column, save/publish, checksum verification, and total time:

```text
screenshot_download_bench_tsv	label	system	variant	encoded_bytes	decoded_bytes	download_ms	decompress_ms	save_ms	verify_ms	total_ms	wire_mbps	decoded_mbps	etag	content_encoding	cf_cache_status	result
```

For identity responses `decompress_ms` is always zero. Any future compressed
artifact experiment should happen outside the MagiK runtime first and only move
back on device if there is clear total-time evidence and a deliberate runtime
decision to carry decoder code.

Screenshot save benchmark rows isolate publish-to-disk cost from download and
checksum work:

```text
screenshot_save_bench_tsv	label	system	mode	iteration	bytes	copy_ms	sync_ms	rename_ms	parent_sync_ms	total_ms	progress_events	result
```

Run the benchmark before changing save behavior:

```bash
scripts/profile-screenshot-save.sh SAVE-PROGRESS-YYYYMMDD --system neogeo --iterations 10
```

Compare average and p95 `total_ms` and `copy_ms`. The progress save path is the
only supported screenshot-pack publish path; record performance evidence in
`history/toolchain-bench/results-screenshot-save.tsv`.

MAME and HBMAME identity metadata are fixed SQLite artifacts. The manual,
main-only game-database workflow publishes both in sequential
`game-databases-vN` releases and rebuilds only upstreams whose tag or revision
changed. This workflow, `.github/workflows/game-databases.yml`, is the only
production path permitted to create a bundle or run `mame-metadata-build`.
Distribution assembly downloads one numbered archive, its manifest, and
`SHA256SUMS` into a release directory and passes only that directory to
`scripts/package-distribution.sh`. The packager fails closed on missing,
ambiguous, mismatched, corrupt, or unsafe bundles before extracting into private
staging. The catalog stamp tracks the verified database file signatures.
Runtime deploy and application publication never build or accept raw database
inputs.

Synthetic SQLite bundles are permitted only as temporary isolated test fixtures
for corruption, mismatch, undersized-data, and traversal coverage. They are not
package defaults, release candidates, or publication inputs.

## SQLite Build And Publish

Production `/media/fat/mister-magik/library.sqlite3` builds use tmpfs by default:

1. Build path: `/tmp/mister-magik/sqlite-build/.library.sqlite3.build.<pid>`.
   The build transaction includes the exact compressed navigation projection
   in `catalog_navigation_projection` row `id=0` and, during the compatibility
   migration, still populates the materialized selector tables.
2. Final temp path beside the DB:
   `/media/fat/mister-magik/.library.sqlite3.tmp.<pid>`.
3. On success, copy/sync the completed tmpfs DB to the final temp path, sync it,
   rename over the production DB, then sync the parent directory.
4. If tmpfs build fails for filesystem-style reasons, the writer falls back to a
   beside-final temp DB. Logical import errors are not retried.

The old DB remains in place until the replacement is complete. The final
publish step uses the same progress-capable chunked save policy as screenshot
packs: copy bytes to the final temp file, sync, rename, and parent-dir sync. The
catalog worker reports this as `Saving library` with byte progress so the build
screen can show a determinate 0-100% saving phase.

Use `scripts/profile-library-save.sh` to isolate the final SQLite publish cost
from discovery and import work:

```bash
scripts/profile-library-save.sh LIBSAVE-YYYYMMDD --iterations 5 --replace-label
```

Rows are appended to `history/toolchain-bench/results-library-save.tsv`:

```text
library_sqlite_publish_tsv	label	iteration	mode	bytes	build_sync_ms	copy_ms	final_sync_ms	rename_ms	parent_sync_ms	total_ms	progress_events	result
```

## Public Read APIs

UI code should read catalog data through the existing `library_db` facade:

- `load_arcade_catalog_from_sqlite`
- `load_amigavision_launch_refs`

Diagnostic tools may still query virtual launch plans directly for benchmark
sample selection, but selected launcher rows must use the structured launch
plans hydrated into `ArcadeCatalog`.

These APIs reject old-schema databases. Callers should treat a schema mismatch
as "cache unusable" and let the worker rebuild.

Runtime Arcade filters use metadata already hydrated into `ArcadeCatalog` rows:
year, manufacturer, and category. Category comes from the offline MAME/HBMAME
metadata DB when present, not from runtime XML or hot-path database queries.
Filter indexes preserve the raw stored/searchable category while normalizing
presentation-only spelling variants. The launcher always offers `Games A-Z`
and `Search`; Decades, Manufacturer, and Categories are offered only when the
active system or virtual collection has at least two distinct choices.

`mister-magik-fb catalog-inspect filter-options [COLLECTION]` loads the same
navigation projection used at startup, compares its filter options with full
SQLite hydration, and prints `catalog_filter_*_tsv` rows. Device acceptance
uses `menu:arcade` to prevent a current-schema but incomplete navigation cache
from silently collapsing the Arcade category list.

Catalog rebuild failures must leave the launcher in an explicit failure state,
not an indefinite progress state. If persistence or post-save catalog loading
fails after the UI has shown `Saving library`, the worker/session path should
replace the progress overlay with a visible failure status such as
`Library load failed` and include the underlying error in the detail text.

## Structured Launch Handoff

Virtual `magik-plan:*` rows are not materialized as temporary `.mgl` files.
Catalog build publishes structured launch descriptors into
`launcher_launch_plans`, and catalog load hydrates them into the runtime
`ArcadeCatalog`.

Selected launch resolves the row in memory:

- Real `.mra`, `.mgl`, and `.rbf` paths continue through
  `mister_magik_launch <path>`.
- Structured virtual rows use
  `mister_magik_launch_plan_v1 <encoded-plan>`.

Launch selection must not query SQLite, read or write generated descriptors,
repair descriptor stamps, or start post-ready descriptor prewarming.
`MiSTer_MagiK` and `mister-magik-fb` are deployed together for this interface.

## Benchmarking

Host checks:

```bash
cargo test --manifest-path magik-gui/catalog/Cargo.toml
cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features
```

Device checks:

```bash
scripts/profile-first-scan.sh LABEL --deploy-device --replace-label
scripts/bench-library.sh LABEL --device --replace-label --iterations 3
scripts/profile-library-io.sh LABEL --replace-label
```

`library-scan-bench` reports:

- `fresh_build`: in-memory scan and classification time,
- `library_scan_timing`: fine-grained scan sub-stage timings in logs,
- `import`: SQLite import/publish time,
- `cached_arcade_load`: runtime catalog read time,
- `root_stamp_check`: warm unchanged validation time,
- `force_rebuild`: optional explicit rebuild when
  `MISTER_LIBRARY_BENCH_FORCE_REBUILD=1`.

Use device evidence before claiming the catalog meets the performance gates.

## Builder boundary

Catalog validation and writes run in-process through the catalog crate's
`builder_service`. The launcher maps typed events directly into its lifecycle.
A cold build publishes
a validated temporary navigation snapshot before durable SQLite publication so
the launcher can become usable at the existing RAM-catalog gate. The existing
SQLite, summary, navigation, stamp, and rebuild-marker formats remain the
on-disk contract.

Normal runtime/platform deploys build and install only the frontend. Build,
deploy, and profile the standalone developer harness for an isolated
catalog-optimization iteration with:

```bash
scripts/build-catalog-builder.sh --device
scripts/deploy-catalog-builder.sh
scripts/profile-catalog-builder.sh LABEL
```

Catalog-only deployment atomically replaces the developer harness and does not
update the platform manifest. Production deployment and packaging hash-pin the
embedded frontend in `platform-v2.manifest`; no standalone builder is shipped.
Runtime folder classification treats names, installed core identities, payload
extensions, and support firmware as evidence. Strong normalized numeric-family
aliases such as `PC88` and `PC8801` may learn the folder's observed payload
extensions; extension-only ambiguity remains unclassified. Firmware such as
`boot.rom` contributes evidence but is never emitted as a game row.
