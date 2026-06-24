# Item 14: Filter SQLite Metadata Loads

## Confirmed Cause

The SQLite import `metadata_load` stage loaded the full MAME machine metadata table on every production import:

- Before: `mame=50097 hbmame=608`.
- The current device catalog only needed metadata for `2975` arcade/Neo Geo setnames after playable discovery de-duplication.
- This over-read allocated tens of thousands of unused machine rows before insert/import work began.

## Change

- Compute the preferred playable discovery set once before metadata loading.
- Collect only arcade/Neo Geo setnames that need MAME/HBMAME enrichment.
- Load MAME and HBMAME machine metadata with chunked `WHERE setname IN (...)` queries for that set.
- Reuse the same preferred discovery set for the insert loop.
- Keep full software-list metadata loading unchanged in this commit.

## Benchmark

Command:

```bash
scripts/profile-library-io.sh ITEM14-BEFORE-metadata-alloc --replace-label --sample-limit 120
scripts/profile-library-io.sh ITEM14-AFTER-metadata-alloc --replace-label --sample-limit 120
```

Artifact:

- `history/toolchain-bench/results-library-io.tsv`

Rows:

```text
ITEM14-BEFORE-metadata-alloc import_stage_metadata_load 1534 mame=50097 hbmame=608 software_lists=14979 preview_paths=8
ITEM14-BEFORE-metadata-alloc import_stage_insert_games_total 2096 rows=9229 launcher_rows=6245
ITEM14-BEFORE-metadata-alloc import_stage_insert_launcher_console 215 rows=6185
ITEM14-BEFORE-metadata-alloc import_stage_build_saved_catalog 117 rows=7256
ITEM14-BEFORE-metadata-alloc refresh_done scan_us=4237840 discover_us=1326701 classify_us=4114533 import_us=6894486 discoveries=9229 normal_files=7897 containers=154 entries=281

ITEM14-AFTER-metadata-alloc import_stage_metadata_load 1168 mame=2671 hbmame=608 mame_needed=2975 software_lists=14979 preview_paths=8
ITEM14-AFTER-metadata-alloc import_stage_insert_games_total 2024 rows=9229 launcher_rows=6245
ITEM14-AFTER-metadata-alloc import_stage_insert_launcher_console 269 rows=6185
ITEM14-AFTER-metadata-alloc import_stage_build_saved_catalog 121 rows=7256
ITEM14-AFTER-metadata-alloc refresh_done scan_us=2617554 discover_us=2019650 classify_us=2493746 import_us=6207050 discoveries=9229 normal_files=7897 containers=154 entries=281
```

Result:

- `import_stage_metadata_load`: `1534ms -> 1168ms`, 366ms faster, 23.9% reduction.
- Target was at least 20% reduction, threshold `<=1227ms`.
- Row-count guards unchanged: `rows=9229`, `launcher_rows=6245`, `launcher_console rows=6185`, `saved_catalog rows=7256`, `discoveries=9229 normal_files=7897 containers=154 entries=281`.

## Validation

```bash
cargo test --manifest-path magik-gui/catalog/Cargo.toml software_identity::tests::mame_machine_metadata_filter_loads_only_needed_setnames -- --nocapture
cargo test --manifest-path magik-gui/catalog/Cargo.toml software_identity::tests::arcade_identity_uses_hbmame_metadata_after_mame_miss -- --nocapture
cargo test --manifest-path magik-gui/catalog/Cargo.toml software_identity::tests::nes_software_identity_matches_title_and_preview_pack -- --nocapture
cargo test --manifest-path magik-gui/catalog/Cargo.toml software_identity::tests::saturn_multidisc_software_identity_materializes_one_launcher_game -- --nocapture
cargo clippy --manifest-path magik-gui/catalog/Cargo.toml --all-targets -- -D warnings
cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings
cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings
```
