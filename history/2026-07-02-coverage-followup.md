# 2026-07-02 coverage follow-up

Scope: host-testable Rust logic. Generated UI code, live UI startup, SSH/device
execution, and Linux-only agent paths remain outside unit coverage unless they
can be moved behind pure helpers.

Commands used:

```bash
/private/tmp/cargo-llvm-cov-install/bin/cargo-llvm-cov llvm-cov --manifest-path magik-gui/Cargo.toml --lib --no-default-features --summary-only
/private/tmp/cargo-llvm-cov-install/bin/cargo-llvm-cov llvm-cov --manifest-path magik-gui/catalog/Cargo.toml --summary-only
/private/tmp/cargo-llvm-cov-install/bin/cargo-llvm-cov llvm-cov --manifest-path tools/mister/Cargo.toml --summary-only
/private/tmp/cargo-llvm-cov-install/bin/cargo-llvm-cov llvm-cov --manifest-path tools/magik-agent/Cargo.toml --summary-only
/private/tmp/cargo-llvm-cov-install/bin/cargo-llvm-cov llvm-cov --manifest-path desktop/Cargo.toml --summary-only
```

Coverage observed before the focused additions:

- `magik-gui`: 89.48% line coverage.
- `mister-magik-catalog`: 89.73% line coverage.
- `tools/mister`: 44.38% line coverage.
- `tools/magik-agent`: 84.44% line coverage.
- `desktop`: 39.38% line coverage.

Focused changes:

- `desktop/src/app_state.rs`: fixed malformed process arrays rendering as
  `1 running ()`; non-numeric PID values are ignored and all-malformed arrays
  now report `unknown`.
- `desktop/src/agent_client.rs`: added tests for token discovery, response
  protocol errors, command errors, agent status formatting, and stale/missing
  MagiK runtime status fallbacks.
- `tools/mister/src/media.rs`: fixed benchmark row parsing so wrong TSV row
  types are rejected, then added tests for TSV sanitizing, manifest URL
  construction, image-size parsing, media index validation, and profile-row
  header writing.

Coverage after the focused additions:

- `tools/mister`: 45.95% line coverage.
- `tools/mister/src/media.rs`: 44.68% to 52.64% line coverage.
- `desktop`: 60.04% line coverage.
- `desktop/src/agent_client.rs`: 32.00% to 67.94% line coverage.
- `desktop/src/app_state.rs`: 88.37% to 94.59% line coverage.

Remaining multi-commit test plan:

1. Add `crash_report` success-path coverage.
   Cover report-helper success output, report directory creation, latest-report
   overwrite behavior, and permission/error fallbacks with temporary fixtures.

2. Add `library_cli` query/reporting coverage.
   Cover CLI formatting for empty result sets, multi-row values, SQLite timing
   rows, and read-only statement rejection beyond the existing inspect tests.

3. Add `catalog_build` orchestration coverage.
   Use a tiny fixture catalog to pin progress events, scan-without-audit handoff,
   and artifact save/report behavior without device or media hot-path work.

4. Extract and test more `tools/mister` SSH command builders.
   Move pure command string construction for media state/status/device helpers
   out from SSH execution functions, then test quoting, paths, and failure
   decision branches without opening network sessions.
