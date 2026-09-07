# Catalog removal audit

Date: 2026-09-07. Audited checkout: `73843ca3e`.

The catalog rewrite left genuine unused modules, obsolete tests and documentation, and a much larger legacy implementation kept reachable by diagnostic commands and shared helpers. Start with approximately **1,469 lines** of high-confidence cleanup. A further **2,239 lines** belong to an independently retireable executable. Another **17,139 lines** form a legacy storage/scanner inventory, not a verified net deletion estimate.

This is a report only. No implementation, test, fixture, command, dependency, or device state was changed. The worktree was clean at the start. Private submodule contents and read-only reference material were excluded.

## Line-count accounting

Counts are physical newline counts, including blank lines, comments and inline tests, from Git-tracked files in `crates/catalog/` at the audited revision. This gives one consistent before/after denominator; it is not the whole repository. The baseline includes Cargo manifests/lockfile and data. No binary-size, performance, or build-time savings are inferred.

| Scenario | Before | Potential after | Saving | Interpretation |
| --- | ---: | ---: | ---: | --- |
| A: strongest cleanup candidates | 94,599 | approximately 93,130 | approximately 1,469 | Includes an eight-line allowance for the retained protocol version declaration/comments |
| A plus B: also retire standalone Arcade prototype | 94,599 | approximately 90,891 | approximately 3,708 | Requires deliberate retirement of a currently callable executable |
| A + B + complete C inventory removal | 94,599 | 73,752 + retained/extracted lines | 20,847 minus retained/extracted lines | Optimistic arithmetic ceiling only; C has live consumers and cannot simply be deleted |

The Rust-only baseline is **86,541 lines**. Scenario A would reduce that to approximately **85,165**, saving **1,376 Rust lines**, plus **93 fixture/document lines**. Module declarations, Cargo target wiring, small caller edits, this report, and documentation rewrites are excluded from savings. Test lines below are already included in file totals; do not add them again. The three scenarios are cumulative, not additive.

## A. Strongest removal candidates

| Candidate | Before | After allowance | Saving | Evidence and required companion edit |
| --- | ---: | ---: | ---: | --- |
| `src/multi_system_projection.rs` | 321 | 0 | 321 | Only its own test calls `bootstrap_global_fixture`; remove its `lib.rs` export and feature attribute |
| `src/incremental_inputs.rs` | 617 | 0 | 617 | Input-fact store/probe API has no external code consumers; remove export and stale comment in `build_progress.rs` |
| `src/builder_protocol.rs` | 440 | 8 | approximately 432 | Retired event protocol has no external code consumers; preserve the format's version value of 4 |
| `src/library_bench.rs` | 6 | 0 | 6 | Compatibility re-export only; callers use `library_db` directly; remove exports in both catalog and app libraries |
| `tests/fixtures/compat/alpha-ef79bbb2-shard3-nav1-rich2/` | 93 | 0 | 93 | Six text files with no tracked consumer found; Git retains historical provenance |
| **Total** | **1,477** | **8** | **approximately 1,469** | Subject to focused compile checks when implemented |

### A1. Unused global-scan bootstrap

[multi_system_projection.rs:28](/Users/nigelb/slint/mister-slint/crates/catalog/src/multi_system_projection.rs:28) takes the old whole-library scan through `CatalogNavigationProjection` and publishes system artifacts. Current production uses fast source adapters instead.

Rust LSP found exactly two references to `bootstrap_global_fixture`: the declaration and the module's own fixture test at line 257. Repository text searches found no outside consumer of the exported outcome/error types either. Its final **89 lines** are test/support code, with **one test**. Remove these with the unused bootstrap, not the production shard/publication tests.

### A2. Unused input-fact database

[incremental_inputs.rs:94](/Users/nigelb/slint/mister-slint/crates/catalog/src/incremental_inputs.rs:94) implements the older input-probe and SQLite fact-store model. The only outside textual mention of its API is a comment in `build_progress.rs:19`. Rust LSP found five references to `InputFactStore`, all inside its own file.

Its final **153 lines**, including **five tests**, exercise the orphan implementation. The current `fast_catalog_refresh` watch/snapshot implementation and tests must remain.

### A3. Retired external-builder messages

[builder_protocol.rs:90](/Users/nigelb/slint/mister-slint/crates/catalog/src/builder_protocol.rs:90) retains handshake, heartbeat, progress, plan, publication, failure, and summary message types. Exported-name searches found no outside consumers of these types. Its final **193 lines** contain **seven protocol tests**.

One real dependency survives: [catalog_format.rs:38](/Users/nigelb/slint/mister-slint/crates/catalog/src/catalog_format.rs:38) reads `CATALOG_BUILDER_PROTOCOL_VERSION`. Move that constant into the format owner, preserving **4** and the serialized `builder_protocol_version` field. Removing that persisted field or changing its value is a separate compatibility change, not cleanup.

`catalog_lease.rs:20` already declares its own lock-path constant; its matching name is not a dependency on this module. Keep the active lease implementation and lock identity. Also keep the current catalog worker's independent child-process protocol.

Together A1–A3 contain **435 lines of trailing test modules and 13 tests** for the obsolete implementations.

### A4. Compatibility re-export

[library_bench.rs](/Users/nigelb/slint/mister-slint/crates/catalog/src/library_bench.rs) only re-exports `library_db::run_scan_bench_with_config`. The app re-exports the module at `apps/mister/src/lib.rs:223`, but no call through this alias was found. This six-line cleanup does **not** retire `library-scan-bench` itself.

### A5. Orphan historical fixture corpus

[fixture README](/Users/nigelb/slint/mister-slint/crates/catalog/tests/fixtures/compat/alpha-ef79bbb2-shard3-nav1-rich2/README.md) says tests should consume the corpus and verify provenance hashes. No tracked consumer of its directory, revision suffix, unique fixture name, navigation filename, or provenance filename was found outside the corpus. The current `catalog_format` tests construct predecessor descriptors directly.

The six files contribute 13 + 6 + 25 + 21 + 24 + 4 = **93 lines**. This is an unused test asset recommendation, not a recommendation to remove predecessor compatibility classification or startup cleanup. Confirm no external fixture consumer before deletion; if the corpus is intentionally retained as reference material, omit these 93 lines from scenario A.

## B. Separate executable-retirement opportunity

The standalone [Arcade prototype](/Users/nigelb/slint/mister-slint/crates/catalog/src/bin/arcade_catalog_prototype/main.rs:5) is still a Cargo binary gated by `builder`. It exposes `compile-base`, `build-active`, `build`, and `inspect`. It is therefore callable tooling, not unreachable code.

| File under `src/bin/arcade_catalog_prototype/` | Lines |
| --- | ---: |
| `builder.rs` | 881 |
| `main.rs` | 403 |
| `model.rs` | 722 |
| `scan.rs` | 233 |
| **Before → after if retired** | **2,239 → 0** |

These files contain **10 tests**, already included in the total. The exported `arcade_catalog_prototype_model` is consumed only by this executable. No tracked invocation was found; the similarly named agent CLI strings are tests asserting that old build intents are rejected.

[tooling-retirement.md:25](/Users/nigelb/slint/mister-slint/docs/tooling-retirement.md:25) explicitly says the other catalog executables were preserved in the prior retirement. Treat deletion as a new scope decision. If retired, remove the Cargo binary declaration and model export together; leave negative CLI tests that prevent old command resurrection. No dependency/lockfile saving has been estimated.

## C. Large legacy path: remove only after resolving consumers

These modules are legacy-dominated, but remain connected. Counts are gross file inventories, including their tests. **None of the following rows means “delete the whole file now.”**

| Storage/sidecar module | Lines | Main obstruction |
| --- | ---: | --- |
| `sqlite_catalog.rs` | 7,519 | Old loaders/writers still called; general SQLite/filesystem helpers are reused |
| `catalog_navigation.rs` | 1,413 | Old loaders and `ArcadeCatalog` conversion depend on its types |
| `catalog_checkpoint.rs` | 955 | Old scan/state/stamp path |
| `catalog_store.rs` | 335 | Old stamp/checkpoint persistence |
| `catalog_state.rs` | 276 | Old scan artifacts and scanner cache |
| `catalog_summary.rs` | 253 | Old SQLite summary path; desktop's same-named function is unrelated |
| `scanner_cache.rs` | 366 | Old scanner plus ROM-identity diagnostic cache-path dependency |
| `catalog_build_record.rs` | 118 | Old completed-build-duration sidecar, consumed by `sqlite_catalog` |
| **Storage subtotal** | **11,235** | Extract retained helpers and resolve old readers first |
| `catalog_build.rs` | 406 | Arcade ROM audit still invokes `CatalogRefreshPipeline` |
| `library_indexer.rs` | 3,306 | Old pipeline, audit, shared attribution type and tests |
| `build_progress.rs` | 1,355 | Indexer's resumable scan journal; configuration path helpers |
| `prepared_bundle_helper.rs` | 837 | Legacy indexer's prepared-target cache support |
| **Scanner subtotal** | **5,904** | Retarget audit before retiring indexer/journal |
| **Combined gross inventory** | **17,139** | Net saving will be lower where code is extracted or retained |

Two mixed files add **4,754 lines of further review surface**, excluded from all savings: `library_db.rs` (4,085) and `library_cli.rs` (669). They contain obsolete wrappers and old-pipeline tests, but also current shared types, normalization helpers, metrics, audits, and retained SQL inspection. Do not delete either wholesale.

### Concrete consumers to resolve

1. **Old scan benchmark:** [library_cli.rs:17](/Users/nigelb/slint/mister-slint/crates/catalog/src/library_cli.rs:17) scans, saves, loads and checks the monolithic SQLite catalog. It is reachable from the auto-discovered `src/bin/library-scan-bench.rs` and the diagnostics app command at [app_entry.rs:894](/Users/nigelb/slint/mister-slint/apps/mister/src/app_entry.rs:894). Retiring it requires removing both entrypoints, command registration/usage tests, and exclusive environment/path plumbing. Keep SQL inspection below line 180 in `library_cli.rs`.
2. **Launch-preparation benchmark:** [launch_preparation.rs:768](/Users/nigelb/slint/mister-slint/apps/mister/src/launch_preparation.rs:768) opens the old SQLite catalog under `bench-tools`. Retarget its input to current system artifacts or retire this benchmark; it is not evidence of a production launcher fallback.
3. **Effects experiment:** [effect_loop_support.rs:379](/Users/nigelb/slint/mister-slint/apps/mister/src/ui_runner/experiments/effects/effect_loop_support.rs:379) loads old SQLite rows to select preview images. Retarget its input or retire the consumer.
4. **Retained Arcade ROM audit:** [library_db.rs:1185](/Users/nigelb/slint/mister-slint/crates/catalog/src/library_db.rs:1185) runs filtered and unfiltered legacy scans. Preserve the audit's visibility comparison while moving discovery onto current adapters. This blocks indiscriminate removal of `catalog_build` and `library_indexer`.
5. **Metadata and ROM diagnostics:** `hbmame-metadata-from-library` reads the old database schema at `library_db.rs:1456`; `software_identity.rs:1702` gets the scanner-cache path for `rom_identity_benchmark_report`. Resolve their intended input contracts rather than silently dropping diagnostics.
6. **Shared helpers:** [atomic_publish.rs:69](/Users/nigelb/slint/mister-slint/crates/catalog/src/atomic_publish.rs:69) calls `sqlite_catalog::sync_parent_dir`; `software_identity` uses SQLite table/temp-path/sync helpers through `library_db`. Move retained helpers before deleting their old owners. Current fast sources also use `library_db::canonical_variant_title` and `normalize_id`.

### Tests to remove versus tests to port

The old `sqlite_catalog.rs` trailing test module alone is **3,155 lines / 61 tests**. The old build journal has **436 lines / 16 tests**. Both are included in C, not additional savings.

Tests exclusively asserting retired sidecar formats, old database repair, old resume journals, and obsolete duration files can disappear with those features. However, source-discovery, metadata, and software-identity tests in otherwise live modules currently round-trip through the old SQLite writer/loader. For example, `catalog_scan.rs:2357`, `media_metadata.rs:1669`, and `software_identity.rs:4087` do this. Preserve their behavioral assertions using current discovery/shard APIs before removing the old harness. Their whole files are not cleanup candidates.

Likewise, the two `ArcadeCatalog::from_navigation_projection` tests at `arcade_catalog.rs:3685` and `:3743` cover search/autocomplete behavior through an obsolete constructor. Retain equivalent behavior coverage through the current loading path if retiring that conversion.

## D. Stale test and documentation debris

### Existing release-boundary script fails against current worker design

Executed:

```text
python3 scripts/tests/test-embedded-catalog-release.py
AssertionError: catalog worker: production still contains 'Command::new'
```

[test-embedded-catalog-release.py:38](/Users/nigelb/slint/mister-slint/scripts/tests/test-embedded-catalog-release.py:38) rejects every `Command::new` in the source, including tests. The current worker intentionally launches the **same executable** with `CATALOG_WORKER_COMMAND` at [catalog_worker.rs:1473](/Users/nigelb/slint/mister-slint/apps/mister/src/ui_runner/catalog_worker.rs:1473); this is distinct from the retired separately installed builder binary.

Recommendation: replace the broad assertion with checks for the retired external builder/service and validate the current self-executable worker boundary. Keep useful release packaging/manifest assertions. No direct tracked reference to this script's filename was found, but that alone does not rule out glob-based or external execution. Do not delete it merely because it currently fails. It is **79 lines**, excluded from savings. Execution stopped at this assertion; later assertions and its generator check were not reached.

### Public architecture page describes the retired system

[catalog-and-preview.mdx](/Users/nigelb/slint/mister-slint/documentation/src/content/docs/architecture/catalog-and-preview.mdx:8) describes RAM-first readiness, monolithic SQLite persistence, `library.summary.json`, and `library.nav.lz4b`. This contradicts [docs/catalog.md](/Users/nigelb/slint/mister-slint/docs/catalog.md:3), which identifies the fast catalog as the sole production architecture.

Replace the page's old diagram and prose with the current source-snapshot → per-system NavPack/search → registry/on-demand read flow. Also fix the stale “fast nine-system catalog” comment at `fast_catalog_sources.rs:4` and `nav_pack.rs` spelling in `docs/catalog.md`'s implementation list (`navpack.rs` is the actual file). These are documentation corrections, with no predicted net line saving.

## Keep: misleading names and still-required boundaries

- `fast_five_catalog`: current production snapshot/artifact format, despite its historical name; prior retirement already removed exclusive experiments.
- `catalog_scan`, `game_discovery`, `catalog_projection`, media identity, launch profiles and prepared collections: current fast adapters still reuse discovery/types/helpers.
- `catalog_acceptance`: current app inspection validates fast snapshots, manifests, NavPacks and search, despite old-looking output labels.
- `catalog_lease`, `predecessor_cleanup`, `legacy_user_state_import`, and compatibility classification: live coordination, upgrade cleanup, and user-data preservation.
- `catalog-hotpath-profile`, its synthetic fixture, and `runtime-metadata-builder`: current fast-build profiling / app PMU support / game-database CI consumers. The standalone Arcade prototype decision does not cover these.
- `rusqlite`, SQLite search databases, and current publication/search tests: only the top-level monolithic SQLite catalog is retired; SQLite itself is still part of the current architecture.
- Current worker child-process supervision, launcher publication tests, negative tests for rejected old CLI commands, and screenshot assets: not legacy debris merely because they mention old names or scanning.

## Recommended implementation order and assurance

1. Implement A as a small deletion change, preserving persisted version and lease identities. Confirm the fixture's reference-only status if it is meant to remain a historical corpus.
2. Repair the stale release-boundary assertion and public documentation separately so they accurately guard/explain current behavior.
3. Decide B independently because the executable was explicitly retained in the last retirement.
4. Resolve C's command/audit consumers, extract shared helpers, and port retained behavior tests. Then delete the obsolete pipeline and measure the actual net diff; do not present the 17,139-line inventory as a guaranteed saving.

For implementation, preview validation with `scripts/agent plan`, run focused catalog checks/tests/Clippy through `scripts/cargo`, and check affected app features (`diagnostics`, `bench-tools`, experiments) as appropriate. For A, include builder and default-feature compilation and the retained `catalog_format`/lease tests. Let CI own broad workspace/host/ARM/visual matrices. No device operation is required to establish source-level removability; device-performance claims would require separate evidence.

Evidence here consists of tracked-file/reference searches, source reads, exact newline counts, the existing script execution above, and Rust LSP reference checks in the correct `rust-main` workspace. Bootstrap and input-store LSP results were complete and untruncated. The event-enum LSP result was truncated; its no-outside-consumer finding relies on the complementary full tracked-source exported-name search, not that partial result. Initial broad shell outputs were truncated and narrowed before using them as evidence. No deletion branch or Rust build was performed, so compiler-verified net removal remains future implementation work. External/private consumers are outside this audit's assurance.
