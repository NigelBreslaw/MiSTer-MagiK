# Item 13: Remove Extra Summary And SQLite Reload

## Confirmed Cause

The production catalog save path performed two redundant post-save hydrations:

- `write_catalog_summary_for_sqlite()` reopened the just-published SQLite database and loaded a full `ArcadeCatalog` only to write `library.summary.json`.
- The catalog worker then reopened SQLite again after `library_db_saved` to send `Ready` to the UI.
- Warm startup also preferred the older `ui_arcade_preferred` projection before `launcher_catalog`, so full catalog hydration did extra query work even though `launcher_catalog` already contains the complete ordered catalog.

## Change

- SQLite import now builds a `LibraryCatalogLoad` from the same transaction after `launcher_catalog` is materialized.
- `library.summary.json` is written from that saved catalog, without reopening SQLite.
- The worker sends the saved catalog directly after persistence/rebuild instead of reloading the final database.
- Warm catalog hydration now prefers `launcher_catalog` and only falls back to older projections when needed.

## Benchmarks

Commands:

```bash
scripts/profile-first-scan.sh ITEM13-BEFORE-summary-reload --skip-build --replace-label --timeout 240
scripts/profile-first-scan.sh ITEM13-AFTER-summary-reload --skip-build --replace-label --timeout 240
scripts/profile-warm-catalog-start.sh ITEM13-BEFORE-summary-reload --replace-label --iterations 3
scripts/profile-warm-catalog-start.sh ITEM13-AFTER-summary-reload --replace-label --iterations 3
```

Artifacts:

- `history/toolchain-bench/results-first-scan.tsv`
- `history/toolchain-bench/results-warm-catalog.tsv`

Cold first scan rows:

```text
ITEM13-BEFORE-summary-reload library_db_saved 53794 ... import_us=12298395 discoveries=9229 normal_files=7897 containers=154 entries=281
ITEM13-BEFORE-summary-reload library_ready 54161 games=7256 load_us=374337
ITEM13-BEFORE-summary-reload db_count 0 9229

ITEM13-AFTER-summary-reload import_stage_build_saved_catalog 229 rows=7256
ITEM13-AFTER-summary-reload library_db_saved 53292 ... import_us=12111083 discoveries=9229 normal_files=7897 containers=154 entries=281
ITEM13-AFTER-summary-reload library_ready 53292 games=7256 load_us=228989
ITEM13-AFTER-summary-reload db_count 0 9229
```

Cold result:

- `library_db_saved -> library_ready`: `367ms -> 0ms`.
- Ready catalog load/build metric: `374337us -> 228989us`, 38.8% reduction.
- Counts unchanged: `db_count=9229`, `games=7256`, `discoveries=9229 normal_files=7897 containers=154 entries=281`.

Warm startup rows:

```text
ITEM13-BEFORE-summary-reload 1 first_frame_ms=35 full_catalog_ready_load_us=473355
ITEM13-BEFORE-summary-reload 2 first_frame_ms=35 full_catalog_ready_load_us=492782
ITEM13-BEFORE-summary-reload 3 first_frame_ms=47 full_catalog_ready_load_us=474661

ITEM13-AFTER-summary-reload 1 first_frame_ms=43 full_catalog_ready_load_us=440834
ITEM13-AFTER-summary-reload 2 first_frame_ms=35 full_catalog_ready_load_us=454621
ITEM13-AFTER-summary-reload 3 first_frame_ms=35 full_catalog_ready_load_us=441342
```

Warm result:

- Median `full_catalog_ready_load_us`: `474661us -> 441342us`, 7.0% reduction.
- First frame remained in range: `35-47ms -> 35-43ms`.

## Validation

```bash
cargo test --manifest-path magik-gui/catalog/Cargo.toml sqlite_catalog::tests::catalog_summary_publish_matches_sqlite_counts -- --nocapture
cargo test --manifest-path magik-gui/catalog/Cargo.toml sqlite_catalog::tests::sqlite_save_materializes_launcher_catalog_variants -- --nocapture
cargo test --manifest-path magik-gui/catalog/Cargo.toml sqlite_catalog::tests::sqlite_remove_deletes_catalog_summary_projection -- --nocapture
cargo test --manifest-path magik-gui/catalog/Cargo.toml sqlite_catalog::tests::sqlite_arcade_load_returns_launchables_beyond_old_cap -- --nocapture
cargo test --manifest-path magik-gui/catalog/Cargo.toml library_db::tests::ram_catalog_from_scan_matches_sqlite_catalog_for_simple_mra_fixture -- --nocapture
cargo test --manifest-path magik-gui/catalog/Cargo.toml library_db::tests::ram_catalog_uses_mame_metadata_families_like_sqlite_catalog -- --nocapture
cargo clippy --manifest-path magik-gui/catalog/Cargo.toml --all-targets -- -D warnings
cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings
cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings
```
