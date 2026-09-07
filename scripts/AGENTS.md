# Host entrypoints

`scripts/magik` delegates to `magik/host/magik`; `scripts/magik-platform` reuses
that runtime. `scripts/magik-ci` owns CI, releases and host-only maintenance.

Preserve command shapes used by sandbox approvals. Bash uses `set -euo pipefail`
and macOS-compatible syntax. Self-tests use temporary fixtures, never the MiSTer.
Generated output belongs in ignored build/results directories or temporary storage.
