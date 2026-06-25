# LibraryIndexer / CatalogRefreshPipeline Metrics

Date: 2026-06-25
Immediate parent: c77413d2c6deba87bcab6582a97d35d28ae14666

## Benchmarks

First scan:

- BEFORE: `scripts/profile-first-scan.sh ARCH-INDEXER-BEFORE --skip-build --replace-label`
- AFTER: `scripts/profile-first-scan.sh ARCH-INDEXER-AFTER --skip-build --replace-label`
- Evidence: `history/toolchain-bench/results-first-scan.tsv`

Library I/O:

- BEFORE: `scripts/profile-library-io.sh ARCH-INDEXER-BEFORE --replace-label`
- AFTER: `scripts/profile-library-io.sh ARCH-INDEXER-AFTER --replace-label`
- Evidence: `history/toolchain-bench/results-library-io.tsv`

## Named Metrics

First scan:

- `first_frame`: before 374 ms, after 443 ms.
- `bootstrap_counter_sustained_climb`: before 948 ms, after 898 ms.
- `scan_stage_walk`: before 36,259 ms, after 37,117 ms.
- `scan_stage_classify_total`: before 38,069 ms, after 38,880 ms.
- `library_scan_complete`: before 39,298 ms, after 40,048 ms.
- `library_ready`: before 50,715 ms, after 51,503 ms.
- `db_count`: before 9,229, after 9,229.

Library I/O:

- `scan_stage_walk`: before 1,997 ms, after 1,954 ms.
- `scan_stage_classify_total`: before 2,572 ms, after 2,609 ms.
- `import_stage_total`: before 4,545 ms, after 4,585 ms.
- `import_stage_materialize_arcade_ui`: before 433 ms, after 445 ms.
- `import_stage_insert_launcher_console`: before 267 ms, after 267 ms.
- `import_stage_build_saved_catalog`: before 120 ms, after 118 ms.
- `refresh_done import_us`: before 6,169,392 us, after 6,055,420 us.
- `refresh_done scan_us`: before 2,709,085 us, after 2,731,632 us.
- `done`: before 45 s, after 45 s.
- Process I/O sample shape remained stable: sample `disk_io_ms=0` throughout both runs, with final process I/O counters in the same range.

## Result

No meaningful regression detected. The small first-scan timing increase is within cold reboot/full-scan run noise, and the device library I/O run is flat overall with a slightly lower `refresh_done import_us`.

## Tests / Clippy

- `cargo test --manifest-path magik-gui/catalog/Cargo.toml catalog -- --nocapture`: passed.
- `scripts/dev-rust test`: passed.
- `scripts/dev-rust check`: passed.
- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features`: passed.
- `cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings`: passed.
- `cargo clippy --manifest-path magik-gui/catalog/Cargo.toml --all-targets -- -D warnings`: passed.
- `cargo test --manifest-path magik-gui/catalog/Cargo.toml`: passed.
- `git diff --check`: passed after normalizing generated `ARCH-INDEXER-*` TSV rows.
