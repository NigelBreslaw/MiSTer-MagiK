# Script layout

The `scripts/` tree contains the Python `scripts/magik-ci` host CI/release
tooling plus the retained `scripts/agent` operational entrypoint.

- `checks/` — static architecture and workflow checks
- `media/` — host conversion and manifest generation
- `release/` — host release-data and packaging tools
- `tests/` — host-local contract tests

`scripts/magik_ci/` owns CI metadata, architecture reports, platform manifests
and bundles, and game-database release archives. It is pinned and checked with
Ruff, ty, and pytest from the repository `pyproject.toml`.

`scripts/agent` is the sole operational and device entrypoint. Device,
ARM-build, deployment, profiling, acceptance, recovery, and scene orchestration
belongs behind its typed Rust operations. `apps/mister/scripts/dev-ui-mac.sh`
is the deliberate non-operational exception: it launches only the local macOS
UI preview and cannot contact or mutate a MiSTer. New shell interfaces in the
operational categories are rejected by the bootstrap-free Python static
assurance in `scripts/magik_ci/assurance.py`.

Validation ownership is defined in root `AGENTS.md`. `scripts/agent plan`
previews checks without executing them. `scripts/agent guidance PATH` reports
source ownership and applicable instructions without compiling Rust.
