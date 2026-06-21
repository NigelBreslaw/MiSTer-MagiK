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
- `magik-gui/src/launcher.rs` owns virtual launch cache stamping and
  materialization.

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

Warm boot with a usable cache:

1. Launcher loads the current-schema SQLite catalog and presents it.
2. After the UI delay, worker runs `CheckStamp`.
3. If the stored stamp matches the current root stamp, the worker reports
   `Unchanged` and does not rebuild.
4. If the stamp is missing, stale, or cannot be checked, the worker emits real
   progress and runs the same full builder used for cold builds.

Explicit refresh:

1. UI or CLI requests `ForceBuild`.
2. The full builder always runs.
3. There is no incremental rescan, preview-only repair, directory manifest
   validation, or file fingerprint validation path.

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

The full-screen scan UI is for foreground first-scan or forced build states where
the user has no current usable catalog.

The bottom-right `scanning...` badge is progress-driven background UI. It appears
only when the worker emits real background progress after a usable catalog is
already visible. It clears on `Unchanged`, `Ready`, `Done`, persistence failure,
scan failure, or load failure.

`CheckStamp` itself should normally be silent. A matching stamp produces timing
and `Unchanged`, not a visible progress badge. A stale or failed stamp check
emits `Library changed` progress before the rebuild, which turns the badge on.

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
Build packs on the host with `scripts/build-console-screenshot-pack.sh`,
`scripts/build-neogeo-screenshot-pack.sh`, or
`scripts/mister preview-cache-build`, then install the resulting fixed artifact.

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

The old DB remains in place until the replacement is complete.

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
