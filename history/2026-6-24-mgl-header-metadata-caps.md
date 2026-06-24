# Item 12: Cap MGL And Header Metadata Reads

## Confirmed Cause

The catalog hot path still did extra metadata work during production scans:

- `.mgl` metadata used an unbounded string read even though the scanner only needs the early `<rbf>` and first `<file>` fields.
- MGL payload coverage could re-open already parsed `.mgl` launchers during playable de-duplication.
- Saturn region inference tried disc-header probing before cheaper filename and folder evidence.
- SQLite import computed Saturn region fallback before using a canonical software-list identity region.
- `.cue` and `.chd` paths are disc containers, not raw Saturn boot sectors, so direct boot-header reads on them were unnecessary.

## Change

- Bound `.mgl` metadata reads to the first 32 KiB.
- Store the resolved MGL covered-payload path on `GameDiscovery` and reuse it for de-duplication.
- Cache the display path string once per discovered file.
- Prefer Saturn filename/folder and software-list identity evidence before header probing.
- Skip Saturn boot-header probes for `.cue` and `.chd` containers.

## Benchmark

Command:

```bash
scripts/profile-library-io.sh ITEM12-BEFORE-metadata-caps --replace-label --sample-limit 120
scripts/profile-library-io.sh ITEM12-AFTER-metadata-caps --replace-label --sample-limit 120
```

Artifact:

- `history/toolchain-bench/results-library-io.tsv`

Rows:

```text
ITEM12-BEFORE-metadata-caps scan_stage_file_discovery 2161 files=7511
ITEM12-BEFORE-metadata-caps scan_stage_classify_total 2561 discoveries=9354 normal_files=7897 containers=154 entries=281
ITEM12-BEFORE-metadata-caps refresh_done scan_us=2704220 discover_us=1950828 classify_us=2560632 discoveries=9229 normal_files=7897 containers=154 entries=281

ITEM12-AFTER-metadata-caps scan_stage_file_discovery 2091 files=7511
ITEM12-AFTER-metadata-caps scan_stage_classify_total 2494 discoveries=9354 normal_files=7897 containers=154 entries=281
ITEM12-AFTER-metadata-caps refresh_done scan_us=2616140 discover_us=1973029 classify_us=2493958 discoveries=9229 normal_files=7897 containers=154 entries=281
```

Result:

- `scan_stage_file_discovery`: `2161ms -> 2091ms`, 70ms faster, 3.2% reduction.
- `scan_stage_classify_total`: `2561ms -> 2494ms`, 67ms faster, 2.6% reduction.
- Counts unchanged: `discoveries=9229 normal_files=7897 containers=154 entries=281`.
- Media directories remained ignored by targeted tests.

## Validation

```bash
cargo test --manifest-path magik-gui/catalog/Cargo.toml media_metadata::tests -- --nocapture
cargo test --manifest-path magik-gui/catalog/Cargo.toml game_discovery::tests::mgl_covered_payload_does_not_get_virtual_duplicate -- --nocapture
cargo test --manifest-path magik-gui/catalog/Cargo.toml software_identity::tests::saturn_multidisc_software_identity_materializes_one_launcher_game -- --nocapture
cargo test --manifest-path magik-gui/catalog/Cargo.toml catalog_scan::tests::scanner_ignores_gamelists_and_screenshot_media_dirs -- --nocapture
cargo test --manifest-path magik-gui/catalog/Cargo.toml catalog_scan::tests::scanner_prunes_arcade_media_and_cores_but_keeps_arcade_game_mras -- --nocapture
cargo clippy --manifest-path magik-gui/catalog/Cargo.toml --all-targets -- -D warnings
cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings
cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings
```
