# Host entrypoints

Python `scripts/magik-ci` owns CI/release processing; operational and device
workflows remain typed Rust behind `scripts/agent`. Do not add competing
operational wrappers.

For the user-approved isolated 2.0 project, `scripts/magik2` is a thin
entrypoint to Python orchestration in `magik2/host/magik2`. The root
`AGENTS.md` 2.0 exception governs its native transport and internal SSH
bootstrap/repair. The legacy-agent and typed-Rust requirements in this file
apply to 1.0 tooling; do not route 2.0 through the legacy CLI.

Preserve command shapes used by sandbox approvals. Bash uses `set -euo pipefail`
and macOS-compatible syntax. Self-tests use temporary fixtures, never the MiSTer.
Generated output belongs in ignored `build/`, `dist/`, `outputs/`, or temporary
storage.
