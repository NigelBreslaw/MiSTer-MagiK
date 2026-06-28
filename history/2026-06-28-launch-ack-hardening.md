# Launch Ack Hardening

Date: 2026-06-28

## Change

- Rust now treats a MagiK launch FIFO write as accepted only after Main reports a
  handoff state in `/tmp/mister-magik/main-status.json`.
- Accepted states are `HandoffToGame` and `Unconfigured`, matching the existing
  release smoke expectations.
- The acknowledgement must be fresh: Rust captures Main's status timestamp
  before writing the FIFO command and only accepts a handoff status with a newer
  `ts_boot_ms`, so stale `HandoffToGame` files cannot acknowledge a later
  command. If the pre-write Main status is missing, the acknowledgement fails
  closed.
- The launcher auto-launch test hook now waits until a selected Arcade row can
  resolve to a game before consuming its one-shot launch request. Summary-ready
  startup can report `catalog_ready=true` before hydrated Arcade rows exist.

## Benchmarks

Launch-prep is intentionally unaffected because the acknowledgement wait lives
after FIFO command write, outside `launch-prep-bench`.

| Label | Scenario | Count | Errors | p50_us | p95_us |
| --- | --- | ---: | ---: | ---: | ---: |
| LAUNCHACK-BEFORE-20260628 | warm | 60 | 0 | 28 | 3001 |
| LAUNCHACK-AFTER-20260628 | warm | 60 | 0 | 28 | 3998 |
| LAUNCHACK-COLD-BEFORE-20260628 | cold | 36 | 0 | 28 | 2962 |
| LAUNCHACK-COLD-AFTER-20260628 | cold | 36 | 0 | 25 | 2320 |

The final warm run preserved p50 but had an AmigaVision exFAT write outlier in
the first iteration, after the launch-return smoke had changed `ags_boot`
content. The cold scenario resets that file before every sample and is the
deterministic write-path comparison; its p95 improved.

## Validation

- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features launcher::tests -- --nocapture`
- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features launcher_loop -- --nocapture`
- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features launch_handoff_session -- --nocapture`
- `scripts/dev-rust check`
- `scripts/device-launch-return-smoke.sh`
- Code review found a stale-ack risk; the fix now requires Main status
  `ts_boot_ms` to advance after the FIFO write. `launcher::tests` includes a
  regression test for stale `HandoffToGame` status.

The first smoke run exposed the auto-launch one-shot race: Arcade opened at the
requested selected row, but no return-state file was written because the hook
had fired before full game rows hydrated. After the one-shot fix, the smoke
wrote return state for row 17, handed off, returned through `load_core menu.rbf`,
restored Arcade row 17, and consumed the return state.
