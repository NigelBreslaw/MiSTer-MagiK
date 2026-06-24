# 2026-6-24 coverage review

Scope: host-testable Rust logic only. Generated UI code, MiSTer device paths,
network probes, and hardware framebuffer flows were not treated as unit coverage
targets.

Commands used:

```bash
cargo llvm-cov --manifest-path magik-gui/Cargo.toml --lib --no-default-features --summary-only
cargo llvm-cov --manifest-path magik-gui/catalog/Cargo.toml --summary-only
cargo llvm-cov --manifest-path tools/mister/Cargo.toml --summary-only
cargo llvm-cov --manifest-path tools/magik-agent/Cargo.toml --summary-only
```

Coverage after the added tests:

- `magik-gui` host library: 90.19% line coverage.
- `mister-magik-catalog`: 89.89% line coverage.
- `tools/mister`: 43.15% line coverage.
- `tools/magik-agent`: 84.44% line coverage.

Focused improvements:

- `catalog_progress.rs`: 69.74% to 98.38% line coverage by covering all
  structured progress phases and malformed legacy progress strings.
- `crash_report.rs`: 42.86% to 58.79% line coverage by covering missing,
  malformed, and blocked filesystem report-helper paths.
- `tools/mister/src/media.rs`: 25.97% to 42.53% line coverage by covering media
  argument parsing, variant normalization, pack selection, and manifest error
  cases.

Remaining high-value gaps:

- `tools/mister` still has low aggregate coverage because much of it is remote
  MiSTer orchestration and process/SSH behavior.
- `library_cli.rs`, `library_db.rs`, and `sqlite_catalog.rs` still have
  uncovered command/reporting and SQLite edge paths.
- `launch_preparation.rs` has uncovered launch-materialization branches, but
  the safer next tests should be fixture-driven rather than broad mocks.
