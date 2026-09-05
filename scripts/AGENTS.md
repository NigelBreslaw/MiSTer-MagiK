# Host entrypoints

Python `scripts/magik-ci` owns CI/release processing; operational and device
workflows remain typed Rust behind `scripts/agent`. Do not add competing
operational wrappers.

Preserve command shapes used by sandbox approvals. Bash uses `set -euo pipefail`
and macOS-compatible syntax. Self-tests use temporary fixtures, never the MiSTer.
Generated output belongs in ignored `build/`, `dist/`, `outputs/`, or temporary
storage.
