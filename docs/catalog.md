# Catalog V2

This document is the current contract for the MiSTer MagiK catalog system. It is
written for future UI and launcher work, so it focuses on lifecycle, public read
APIs, progress states, and benchmark expectations.

## Goals

- Warm boot with a usable catalog should show the first usable UI within 3.5s.
- Warm unchanged validation should be a root stamp check: under 500ms is the
  soft target, under 2s is the hard gate.
- Fresh catalog creation and explicit refresh both use the same full builder.
  Current acceptance is catalog ready under 60s on the target MiSTer; 45s remains
  a stretch target for future scanner/import work.
- Unchanged virtual launch cache materialization should complete under 2s and
  must not read every generated `.mgl` file.

## Files And Owners

- `magik-gui/catalog/src/catalog_config.rs` owns default roots, DB paths, schema
  version, and catalog build version.
- `magik-gui/catalog/src/catalog_stamp.rs` owns the warm validation stamp.
- `magik-gui/catalog/src/catalog_store.rs` owns stamp persistence helpers.
- `magik-gui/catalog/src/library_db.rs` remains the compatibility facade for
  scanning, classification, SQLite build/publish, and public read APIs while the
  catalog modules continue to split out.
- `magik-gui/src/ui_runner/catalog_worker.rs` owns launcher worker scheduling
  and progress messages.
- `magik-gui/src/launcher.rs` owns the library rebuild-on-next-boot marker plus
  virtual launch cache stamping and materialization.

## Lifecycle

Cold or reset database:

1. Launcher starts immediately and presents Slint UI.
2. Catalog worker treats missing, empty, or old-schema DBs as unusable.
3. Worker runs `ForceBuild`.
4. Full-screen scan UI is visible while the database is built.
5. The builder scans source game locations under `/media/fat`, keeps scan facts
   in Rust memory, creates SQLite under `/tmp/mister-magik/sqlite-build` for
   production `/media/fat` databases, and publishes the completed file at the
   end.
6. The worker reports `Ready` as soon as the saved SQLite catalog has been
   loaded. Virtual launch cache materialization runs after readiness so it
   cannot extend first usable catalog time.

The Settings-screen `Reset Database` action removes the SQLite catalog and all
recognized screenshot pack files under `/media/fat/mister-magik/assets` before
requesting the supervised reboot. It deletes size-qualified and legacy
`<system>-screenshots*.mmlz4b` files for supported pack systems plus
`.screenshot-media-state.json`; unrelated files in the assets directory are
left alone.

Warm boot with a usable cache:

1. Launcher loads the current-schema SQLite catalog and presents it.
2. After the UI delay, worker runs `CheckStamp`.
3. If the stored stamp matches the current root stamp, the worker reports
   `Unchanged` and does not rebuild.
4. If the stamp is missing, stale, or cannot be checked, the worker reports
   `Changed` and exits. It must not run the full builder automatically.
5. The launcher shows a `Library changed` confirmation dialog. `Continue` keeps
   the current catalog for this session and writes
   `/media/fat/mister-magik/rebuild-on-next-boot`. `Rebuild` immediately starts
   a foreground `ForceBuild`.
6. On the next MagiK boot, the launcher consumes the rebuild marker as a
   one-shot request and starts the foreground `Updating Library` flow instead of
   delayed ready-cache validation.

Explicit refresh and chosen rebuild:

1. UI, marker boot, or CLI requests `ForceBuild`.
2. The full builder always runs.
3. There is no incremental rescan, preview-only repair, directory manifest
   validation, or file fingerprint validation path.

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

`CatalogWorkerRequest` has exactly three modes:

- `LoadOnly`: use the already loaded cache and do no background validation.
- `CheckStamp`: validate the ready cache after the UI is usable.
- `ForceBuild`: run the full builder regardless of cache state.

Missing, empty, and old-schema caches are not usable. They always plan
`ForceBuild`, even when the request was `CheckStamp`, unless
`MISTER_CATALOG_REFRESH=off` has disabled the catalog worker for a benchmark
restart.

`MISTER_CATALOG_REFRESH` is the single launcher policy knob:

- unset: normal behavior; ready caches get delayed `CheckStamp` validation,
- `on`/`force`: force a full catalog rebuild,
- `off`/`load-only`: use only the synchronous cache load and start no catalog
  worker.

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

## Root Stamp

The root stamp is the cheap unchanged validation. It includes:

- catalog schema version and catalog build version,
- launch profile set version,
- configured roots and root metadata,
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
except for source-defined special suffixes such as PCE-CD and NeoGeo-CD. Generic
core extension strings come from the core OSD config string at runtime; MagiK
therefore keeps those as explicit launch profiles with provenance instead of
burying guesses in the walker.

The default scan does not include `_Games`. That tree is treated as an organizer
mirror of generated `.mgl` launchers for games already available through
source/core game directories. Set `MISTER_LIBRARY_ROOTS` explicitly for a
diagnostic build that includes `_Games`.

## Supported Launch Profiles

The supported built-in catalog profile set is intentionally fixed. Additions
must update `PROFILE_SET_VERSION`, tests, and this document.

- Launcher profiles: MRA, MGL, and DOS installed MGL launchers.
- Disc/computer profiles: Saturn, PlayStation/PSX, AO486, and Amiga/Minimig.
- Arcade/console profile: NeoGeo.
- Cartridge profiles: NES, SNES, GBA, Game Boy Color, Game Gear, Sega Master
  System, Mega Drive, and Nintendo 64.

These profiles are source-backed by Main_MiSTer behavior, MRA/MGL launch files,
or explicit MagiK profile rules. Generic extension guesses are not a supported
product path.

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
loads LZ4-block `.mmlz4b` packs from `/media/fat/mister-magik/assets`; the
catalog stores only the pack path and asset key needed to request a preview.
Raw preview archive formats and ad hoc on-device pack generation are retired.
Build and publish packs from the sibling `../magik-cloud` repo; this repo keeps
only runtime preview loading, catalog projection, and device acceptance checks.
See `../magik-cloud/docs/media-build.md` for the media-build workflow.

Remote screenshot-pack updates are manifest-driven. On-device MagiK and the
host commands `scripts/mister media-check` and `scripts/mister media-download`
read the Cloudflare R2 manifest from `MISTER_MEDIA_MANIFEST_URL`,
`--manifest-url`, or the default
`https://assets.mistermagik.com/mister-magik/v1/manifest.json`.

Runtime downloads save new packs with the image size in the filename:

```text
/media/fat/mister-magik/assets/arcade-screenshots-320x320.mmlz4b
/media/fat/mister-magik/assets/neogeo-screenshots-320x320.mmlz4b
/media/fat/mister-magik/assets/saturn-screenshots-320x320.mmlz4b
```

Legacy catalog paths such as `arcade-screenshots.mmlz4b` remain valid lookup
keys. The preview worker resolves those legacy paths through the media state to
the preferred size-qualified pack and falls back to legacy fixed-name files when
needed. Current public packs are `320x320`; future smaller packs must preserve
their size in the local filename.

The downloader compares the expected SHA-256 with the raw manifest object and
publishes verified packs atomically. The state file
`/media/fat/mister-magik/assets/.screenshot-media-state.json` records the last
successful media update, preferred size, and the latest HTTP/cache headers
observed for each downloaded pack. It is not a catalog stamp input.

During catalog scans, the scanner emits the first discovery of each supported
screenshot-pack system: `arcade`, `neogeo`, `nes`, `snes`, `n64`, `sms`,
`megadrive`, and `saturn`. The launcher starts the runtime media worker on the
first discovered supported system and queues only those discovered systems.
Unrequested packs are not checked or downloaded. The worker fetches the
manifest once, de-duplicates system requests, and runs at most three pack
downloads concurrently.

`MISTER_MEDIA_UPDATE=off` disables the media worker,
`MISTER_MEDIA_UPDATE=check` reports status without downloading, and
`MISTER_MEDIA_UPDATE=download` is the default. `MISTER_MEDIA_SIZE` defaults to
`320x320`. Progress is emitted as structured `screenshot_media_progress` startup
events with system, size, phase, byte counts, pack index/count, and optional
download Mbps. The catalog-build screen renders up to three compact active pack
rows from the same events. Download and save phases show byte progress; verify,
sync, rename, and parent sync are phase-only unless future evidence shows that
more granularity is needed.

The production path is the canonical `.mmlz4b` object served with
`Accept-Encoding: identity`. Runtime v1 uses manifest `compression: "none"`.
MagiK does not decode or benchmark gzip/Brotli screenshot objects on device.
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

MAME and HBMAME identity metadata are fixed SQLite artifacts. Build them
offline with `scripts/mister mame-metadata-build`, include them in the release
package with `scripts/package-distribution.sh`, and let the catalog stamp track
their file signatures. Runtime deploy no longer builds or copies those metadata
databases; changing them is a catalog/media artifact publish step.

## SQLite Build And Publish

Production `/media/fat/mister-magik/library.sqlite3` builds use tmpfs by default:

1. Build path: `/tmp/mister-magik/sqlite-build/.library.sqlite3.build.<pid>`.
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
- `load_virtual_launch_plan`
- `load_virtual_launch_plans`
- `load_virtual_launch_plans_for_system`
- `load_amigavision_launch_refs`

These APIs reject old-schema databases. Callers should treat a schema mismatch
as "cache unusable" and let the worker rebuild.

## Virtual Launch Cache

Virtual launch files live in `/media/fat/mister-magik/launch-cache`.

The cache stamp file is `.virtual-launch-cache.json`. It records a schema
version, plan count, and a stable fingerprint of each generated basename plus
generated `.mgl` content.

If the stored stamp matches the expected stamp, cache materialization returns a
fast unchanged summary and does not read each generated `.mgl` file. If the stamp
is missing or stale, the existing files are materialized and the stamp is written
only when there are zero errors.

Launch-time behavior is unchanged: a missing virtual launch file is still
created on demand. Worker prewarming is best-effort background maintenance after
catalog readiness, not a prerequisite for browsing games.

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
