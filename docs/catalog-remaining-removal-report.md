# Remaining catalog legacy removal plan

Implementation is now in progress. See [current implementation status](catalog-retirement-status.md);
the inventory and estimates below describe the earlier report-only snapshot.

Date: 2026-09-07. Base: fetched `origin/main`, `8dc2908e7`. Branch: `nigel/catalog-legacy-cleanup`. Scope: the current, partially cleaned worktree at `/private/tmp/mister-magik-catalog-cleanup`.

This supersedes the retention assumptions in `catalog-removal-audit.md`. The earlier report found the large legacy subsystem but incorrectly treated its diagnostic callers as reasons to keep it. This scan follows those callers to their entrypoints and classifies them too. **No further implementation deletions were made during this report-only rescan.**

## Bottom line

The old monolithic SQLite/catalog-v3 builder is still largely present. It is not the current production build path. Retire it as a connected subsystem, together with obsolete commands and tests, rather than deleting only individually unreferenced functions.

The remaining retirement surface is **23,230 physical lines** in the catalog crate: 16 legacy-dominated modules, the mixed `library_db.rs`, and the old scan benchmark binary. Most can go; shared helpers, migration cleanup and meaningful behavioral tests must be extracted or rewritten. A reasonable **planning target is 20,000–22,000 additional net lines removed**, not a compiler-verified saving. The exact net must be measured after that extraction and test porting. Further small app/host/config deletions are listed below but excluded from this estimate.

Important new findings beyond the first audit:

- `catalog_stamp.rs` is another legacy-only dependency chain, not the fast refresh fingerprint owner.
- Most of `catalog_projection.rs` can go; the new source adapters need its title canonicalization helper, not the old SQL projection machinery.
- The entire `library_cli.rs` can be retired after its scan benchmark: its supposedly retained SQL command is also an orphan facade. Current host SQL queries use `sqlite_inspect.rs` directly.
- The ROM visibility audit explicitly compares old scanner output and fast candidates. Its old comparison is migration scaffolding, not a reason to retain the old scanner indefinitely.
- Host `catalog query` still advertises two obsolete database targets beneath the new catalog root.
- Old load counters keep tests appearing useful even though they do not instrument the new per-system I/O path.

## Before and potential after

Counts include blank lines, comments and inline tests. The denominator is Git-tracked files under `crates/catalog/`, not the whole repository. Counts were read from `HEAD` and the current filesystem; deleted files count as zero. This report, the app validation probe and other out-of-crate changes are excluded. The existing partial cleanup is uncommitted.

| State / scenario | Catalog lines | Saving from current state |
| --- | ---: | ---: |
| Original fetched base | 94,599 | — |
| Current partial cleanup | 90,402 | Already 4,197 below base |
| Remove the 16-module gross envelope, before extraction | 70,926 + retained/replacement lines | 19,476 minus retained/replacement lines |
| Also retire old portions of `library_db.rs` and benchmark binary | 67,172 + retained/replacement lines | 23,230 minus retained/replacement lines |
| Planning range after preserving shared code and porting tests | approximately 68,402–70,402 | approximately 20,000–22,000 |

The planning range reserves **1,230–3,230 lines** from the expanded gross envelope for retained/extracted code and replacement tests. This is an explicit engineering allowance, not a measured implementation result. It implies approximately **24,197–26,197 total lines saved from the original base**. Do not add table rows together.

Rust-only counts currently stand at **86,541 → 82,442**, a reduction of **4,099**. No binary-size, performance or build-time saving is inferred from any line count.

## 1. Retire the old subsystem as a unit

Paths in the following inventory are relative to `crates/catalog/src/`. All counts are current, not the earlier audit's pre-cleanup figures. File totals already include tests.

| Module | Lines | Disposition |
| --- | ---: | --- |
| `sqlite_catalog.rs` | 7,523 | Delete old schema/import, relational loaders, embedded navigation recovery, projection repair, preview-flag updates and old publication code. Extract migration deletion and small SQL/filesystem helpers first. |
| `catalog_navigation.rs` | 1,413 | Delete the old whole-catalog navigation serialization and adapters. This is not the current NavPack implementation. |
| `catalog_checkpoint.rs` | 955 | Delete old whole-scan checkpoint persistence and drift reconstruction with its callers. |
| `catalog_store.rs` | 335 | Delete the old catalog-state store with its callers. |
| `catalog_state.rs` | 276 | Delete old scan-state persistence; preserve any required path spelling in the migration/cache owner. |
| `catalog_summary.rs` | 253 | Delete old SQLite summary reading/writing; retain its retired filename only in migration cleanup. Desktop's similarly named functions are unrelated. |
| `scanner_cache.rs` | 366 | Delete old discovery-history cache implementation. Decouple the ROM-identity benchmark's cache-path dependency first. |
| `catalog_build_record.rs` | 10 | Move the remaining retired sidecar filename into migration cleanup, then delete the module. The unused duration reader/writer has already been removed with approval. |
| `catalog_build.rs` | 362 | Delete `CatalogRefreshPipeline` after removing the benchmark and old ROM-audit comparison. |
| `library_indexer.rs` | 3,322 | Delete old whole-library orchestration, durable resume, reused-prefix paths and old attribution machinery. Port retained discovery assertions off its test harness. |
| `build_progress.rs` | 1,354 | Delete old `build-progress-v3` / `target-output-cache-v3` journal, framing and tests. The fast refresh owner does not call it. |
| `prepared_bundle_helper.rs` | 837 | Delete the old indexer's prepared-target cache helpers. Keep the separate current prepared-collection adapters. |
| `catalog_stamp.rs` | 487 | Delete old root stamp/fingerprint computation and its tests. Current refresh snapshots are owned elsewhere. |
| `catalog_projection.rs` | 1,209 | Extract `canonical_variant_title` and its annotation stripping, then delete old SQL materialization, deduplication/projection structures and legacy-only tests. Port still-relevant behavior assertions. |
| `catalog_load_metrics.rs` | 105 | Remove old-loader counters after replacing the app's old no-load test guard with current-I/O coverage. |
| `library_cli.rs` | 669 | Retire scan benchmark and orphan on-device SQL facade, including their private implementation/tests. Keep `sqlite_inspect.rs`. |
| **16-module envelope** | **19,476** | Gross surface, not unqualified net deletion. |
| `library_db.rs` | 3,745 | Delete obsolete scan artifacts, preparation pipeline, stamps, SQLite facades and old tests; retain/extract shared archive types and utilities. |
| `bin/library-scan-bench.rs` | 9 | Delete the auto-discovered Cargo entrypoint together with the app command. |
| **Expanded envelope** | **23,230** | Non-overlapping current file totals. |

### Why these references do not justify retention

The benchmark starts in [library_cli.rs:17](/private/tmp/mister-magik-catalog-cleanup/crates/catalog/src/library_cli.rs:17), scans through `LibraryIndexer`, saves the old SQLite schema, reloads it and checks the old stamp. Those calls establish a legacy command chain, not a production dependency. The app still exposes it at [app_entry.rs:894](/private/tmp/mister-magik-catalog-cleanup/apps/mister/src/app_entry.rs:894).

The old navigation constructor at [arcade_catalog.rs:962](/private/tmp/mister-magik-catalog-cleanup/crates/catalog/src/arcade_catalog.rs:962) is referenced by the legacy SQLite loader/repair path and legacy navigation tests. Remove that constructor and its private adapter with the old serialization; preserve current navigation/search behavior tests through current catalog construction.

The durable-journal path helpers at [catalog_config.rs:186](/private/tmp/mister-magik-catalog-cleanup/crates/catalog/src/catalog_config.rs:186) lead back to the old indexer. They are not evidence that the fast builder uses the journal. Delete them with the journal, including their stale lifecycle comments.

## 2. Remove or retarget the entrypoints that keep it alive

| Entrypoint / caller | Recommendation | Evidence / companion work |
| --- | --- | --- |
| `library-scan-bench` | Delete | Remove both Cargo binary and app dispatch, `command_args` registration/exclusivity/tests, `MISTER_LIBRARY_BENCH_*` settings and benchmark-only layout plumbing. This measures the retired builder. |
| `preview-index-refresh-bench` | Delete | [app_entry.rs:1282](/private/tmp/mister-magik-catalog-cleanup/apps/mister/src/app_entry.rs:1282) calls the old SQLite row updater. Current preview availability and fast refresh remain; this old benchmark does not implement them. |
| `hbmame-metadata-from-library` | Delete old on-device command | [library_db.rs:1188](/private/tmp/mister-magik-catalog-cleanup/crates/catalog/src/library_db.rs:1188) reads old `launch_plans JOIN games` to write metadata. Preserve the actual host/CI source-metadata → compact-runtime-metadata build, not this old database-derived route. |
| Old part of `catalog-rom-audit` | Delete old-vs-filtered scanner comparison; retain useful current ROM-health reporting | [library_db.rs:975](/private/tmp/mister-magik-catalog-cleanup/crates/catalog/src/library_db.rs:975) performs two legacy scans and compares them with fast candidates. Replace this with current-source eligibility/reporting if retaining the command. Remove old-only/parity output fields and update consumers. |
| `launch-prep-bench` | Keep preparation benchmark, replace its catalog input | [launch_preparation.rs:768](/private/tmp/mister-magik-catalog-cleanup/apps/mister/src/launch_preparation.rs:768) loads old SQLite; line 969 separately queries old AmigaVision refs. Read current system artifacts/launch plans for both. Do not delete actual launch preparation. |
| Effects experiment's preview selection | Replace only old catalog input | [effect_loop_support.rs:379](/private/tmp/mister-magik-catalog-cleanup/apps/mister/src/ui_runner/experiments/effects/effect_loop_support.rs:379) loads old SQLite to choose preview assets. Use current rows or explicit fixtures. Retiring the whole effects experiment would be a broader decision, unnecessary for catalog removal. |
| On-device `library-sql` facade | Delete | `library_db::run_sqlite_inspect_cli` has no tracked caller; the remaining implementation is called by its own tests. The app command table no longer exposes it. |
| Host `catalog query --database registry/library` | Remove obsolete selectors, retain `system:ID` SQL queries | [host/mod.rs:8756](/private/tmp/mister-magik-catalog-cleanup/agent-cli/src/host/mod.rs:8756) maps these to `state/catalog-state.sqlite3` and `state/scanner-cache.sqlite3` under `catalog-fast-v1`. Neither is the fast builder's registry. Update argument errors and stale test example at line 13194. Use current registry reporting for manifest inspection. |
| Host old SQL fallback helper | Delete helper and its obsolete test | [remote.rs:162](/private/tmp/mister-magik-catalog-cleanup/agent-cli/src/host/remote.rs:162) is already `cfg(test)` and used solely to test recognition of an unavailable `library-sql` command. Preserve generic remote execution/error handling and quoting tests. |

The fast candidate audit helper currently converts a scan failure into an empty vector with `unwrap_or_default()` at [fast_catalog_sources.rs:730](/private/tmp/mister-magik-catalog-cleanup/crates/catalog/src/fast_catalog_sources.rs:730). If used as the replacement ROM-health report, make failure explicit rather than reporting an empty healthy inventory. The production scanner itself must remain unchanged by that reporting adjustment.

## 3. Extract the small genuine dependencies, not their old owners

- **Migration cleanup:** [predecessor_cleanup.rs:82](/private/tmp/mister-magik-catalog-cleanup/crates/catalog/src/predecessor_cleanup.rs:82) calls `sqlite_catalog::remove_catalog_artifacts_at`. Move bounded deletion of the retired database, sidecars, journal/WAL/temp files and ready snapshots into the migration owner. Preserve its fixed-root, symlink and unrelated-file protection tests. Old installed files can outlive old code.
- **Shared SQLite/filesystem utilities:** `atomic_publish` needs parent-directory syncing. `software_identity` needs read-only opens, table/column checks, temporary paths, file signatures and syncing. Extract these small helpers; do not keep the 7,523-line old database implementation to house them.
- **Shared normalization:** [catalog_projection.rs:402](/private/tmp/mister-magik-catalog-cleanup/crates/catalog/src/catalog_projection.rs:402) through line 427 contains the two title-normalization helpers needed by fast source adapters. Move them with focused behavioral coverage. `library_db::normalize_id` is also live.
- **Archive/domain utilities:** `LibraryContainer`, `LibraryContainerEntry`, `ArchiveFormat`, `ArchiveScanStatus`, AmigaVision listing/constants, path/title normalization, endian reads, ZIP EOCD discovery, hashing and timestamps still have live consumers. Move them from `library_db` to small appropriately owned modules, or retain a much smaller module. Do not delete parsers to make the file disappear.
- **ROM benchmark cache path:** `software_identity::rom_identity_benchmark_report` calls `scanner_cache::default_path` at line 1702. Preserve its standalone ROM-identity work, but remove its assumption that the old discovery cache is production authority. Keeping a filename/path helper does not require keeping scanner state persistence.
- **Format compatibility:** Keep existing persisted descriptor fields and version values in `catalog_format`, including builder/state/cache version fields, until an explicit format migration. A field can remain part of the active compatibility identity even after its historical implementation is gone.

## 4. Test debris: delete implementations' tests, preserve behavior coverage

The 16-module envelope contains **163 `#[test]` functions**; `library_db.rs` contains another **24**. These **187 tests are an inventory, not a recommendation to delete all 187**. Tests and test-tail line counts are already included in the source totals.

Delete tests whose sole contract is the retired schema, summary/navigation file encoding, old checkpoint/resume journal, old CLI syntax or old projection repair. Do not recreate these just to maintain a test count.

Port meaningful source/metadata/launch assertions that currently use the old scan + SQLite round-trip as a convenient harness:

- `catalog_scan.rs` imports the old writer/loader at line 2093 and exercises them in archive/discovery tests, including line 2359 onward.
- `media_metadata.rs:1167` imports the same old round-trip helpers.
- `software_identity.rs:2346` imports old SQL writer/test machinery; later tests also exercise old library-derived HBMAME output.
- `library_db` tests mix obsolete preparation/resume parity with meaningful MRA visibility, family metadata, previews and raw-archive launchability assertions.
- The old projection tests include variant/preview behavior worth retaining against current source adapters, not SQL materialization.
- `test_support.rs` constructs `LibraryScan` and old `CatalogScanAttribution`; simplify it after porting its consumers rather than retaining the whole indexer for fixture construction.

The app's [assert_no_catalog_loads_during](/private/tmp/mister-magik-catalog-cleanup/apps/mister/src/launcher.rs:6320) watches counters incremented by old SQLite/summary/navigation loaders. After removal they would be permanently zero. Replace that guard with instrumentation or assertions covering current lazy shard reads; preserve the UI interaction tests themselves.

## 5. What is not legacy debris

- `fast_catalog_refresh`, `fast_catalog_sources`, `generic_system_catalog`: current source discovery, fresh/incremental planning and publication.
- `fast_five_catalog`: its name is historical, but `build_staged_system_artifacts` is still called by the current refresh at line 375. Do not delete it based on the name.
- `system_shard`, `navpack`, `shard_registry`, `lazy_sharded_reader`, `sharded_catalog`, `persisted_search`: current per-system format, registry, loading and SQLite search. SQLite itself and `rusqlite` remain necessary.
- `catalog_acceptance`: current integrity checks still emit a `catalog_v3_summary_tsv` label at line 231. The label is stale naming, not a retired implementation. Renaming it would also require updating report parsers.
- `catalog_discovery`, `catalog_scan`, `game_discovery`, `launch_profiles`, `prepared_collections`, `media_metadata`, `software_identity`, `runtime_metadata`: contain live parsers, source adapters or metadata consumers. Remove proven old-only slices, not whole files.
- `sqlite_inspect`: current host `catalog_query` calls `sqlite_query_to_tsv` directly at `agent-cli/src/host/mod.rs:8610`.
- Catalog lease, predecessor cleanup, current user state/import behavior and persisted format compatibility guards.
- Source metadata databases used by the host/CI compact metadata builder. They are distinct from the retired monolithic launcher database.

Historical evidence under `history/`, private submodules and read-only reference material were not included as bulk deletion opportunities. Generated validation logs/catalog captures are ignored artifacts, not source LOC savings.

## 6. Recommended implementation order and acceptance

1. Preserve the captured baseline and retire obsolete app/CLI entrypoints; retarget launch-preparation and effects inputs. Replace the ROM audit's legacy comparison with current reporting.
2. Extract migration deletion, shared SQL/filesystem utilities, normalization and archive types. Keep persisted descriptor values unchanged.
3. Port behavioral tests to current discovery/system artifacts, then remove the entire old builder/state/projection chain and its exclusive fixtures/tests.
4. Remove stale host selectors, fallback tests, environment/path plumbing and obsolete no-load counters. Re-scan all feature-gated callers and aliases; do not rely on a single default-feature build.
5. Run focused catalog tests/checks with default and builder features, relevant app diagnostics/bench/experiment consumer checks and affected host tests. Let CI own broad matrices.
6. Rebuild a fresh catalog from the same installed source corpus using the cleaned binary. Compare system metadata, counts, ordered full-row/variant fingerprints and integrity report digests for identities, ordering, launch plans, search and the artifact set. Any mismatch must be explained and corrected, not waived as cleanup.

The fresh before capture already contains **92 systems, 35,216 games and 350 variants**, from the pinned original catalog implementation with an opt-in isolated validation probe. Evidence: `build/magik2-results/20260907T084507Z-cdc15132fe9a/catalog.json`. This rescan did not run an after capture, and **does not claim before/after equality**. Refresh the baseline again if installed games or source metadata change before the eventual comparison. Counts alone are insufficient proof.

## Evidence limits

The scan used Git-tracked source searches, source/entrypoint inspection and the repository Rust LSP skill. Rust analysis routed to `rust-main` in this worktree; a complete result confirmed the durable-progress path's indexer dependency. Other queries had feature-limited results, output truncation or position fuzzing onto unrelated symbols; those were not treated as no-consumer proof. Cross-feature conclusions above use explicit source references and dispatch inspection instead. Textual false positives such as `catalog_state` log fields, `catalog_build` capsule fields and desktop `catalog_summary` methods were excluded.

This is a removal plan, not compiler-verified deletion of the remaining subsystem. No new compilation, device mutation or implementation cleanup was performed for this rescan. External/private consumers are outside its assurance. Exact net line savings and behavioral equivalence remain acceptance gates for the implementation.
