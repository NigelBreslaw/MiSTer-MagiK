# Catalog V2

This document is the current contract for the MiSTer MagiK catalog system. It is
written for future UI and launcher work, so it focuses on lifecycle, public read
APIs, progress states, and benchmark expectations.

## Goals

- Warm boot with a usable catalog should show the first usable UI within 3.5s.
- Warm unchanged validation should be a root stamp check: under 500ms is the
  soft target, under 2s is the hard gate.
- Fresh catalog creation and explicit refresh both use the same full builder and
  should complete under 45s on the target MiSTer.
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
5. The builder scans source media under `/media/fat`, keeps scan facts in Rust
   memory, creates SQLite under `/tmp/mister-magik/sqlite-build` for production
   `/media/fat` databases, and publishes the completed file at the end.

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
`ForceBuild`, even when the request was `CheckStamp`.

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
- immediate child directory metadata for each root,
- MAME and HBMAME metadata DB signatures,
- active preview pack signatures.

The stamp intentionally does not walk every file and does not protect against
adversarial same-size/same-mtime edits inside unchanged directories. Normal
copy/update workflows are expected to update root or immediate system directory
metadata. If that assumption is wrong for a workflow, explicit refresh remains a
full rebuild.

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
created on demand.

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
- `import`: SQLite import/publish time,
- `cached_arcade_load`: runtime catalog read time,
- `root_stamp_check`: warm unchanged validation time,
- `force_rebuild`: optional explicit rebuild when
  `MISTER_LIBRARY_BENCH_FORCE_REBUILD=1`.

Use device evidence before claiming the catalog meets the performance gates.
