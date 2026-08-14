# Agent development guide

Use these documents to reduce repository-wide exploration:

- [`task-map.md`](task-map.md) — where to start and which assurance boundary
  owns common work.
- [`file-authority.md`](file-authority.md) — whether a file is hand-edited,
  generated, private, or device-owned.
- [`ai-efficiency.md`](ai-efficiency.md) — bounded inspection, tool-output
  reduction, and privacy-safe context measurement.

Universal safety and workflow rules remain in the root `AGENTS.md`; the nearest
scoped `AGENTS.md` adds subsystem rules.

## Optional Rust analyzer prerequisites

The repo-scoped Codex integration expects `lspi` 0.2.0 on `PATH` and Rust
1.97.1 installed through `rustup` with the `rust-analyzer` and `rust-src`
components. It deliberately launches the analyzer through `rustup`, bypassing
shell tool shims. These dependencies are optional: when they are unavailable,
Codex can still open and edit the repository, but Rust edit-time semantic
assurance is reduced until pre-push and CI run.
