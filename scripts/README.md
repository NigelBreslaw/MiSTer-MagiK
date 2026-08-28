# Script layout

The `scripts/` tree contains the Python `scripts/magik-ci` host CI/release
tooling plus the retained `scripts/agent` operational entrypoint.

- `bench/analyze/` and `bench/reports/` — offline evidence analysis
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
operational categories are rejected by the typed
`BuiltinOperation::ShellOwnership` assurance in `agent-cli`.

Normal repository work uses bounded Rust analyzer diagnostics where applicable,
explicit-path `git add`, ordinary `git commit`, and `git push`; the pre-commit
hook runs the bootstrap-free Python fast gate under its ten-second deadline.
The pre-push hook enters `agent-cli` for full affected assurance before
committed work reaches the remote. Pre-push and CI run every selected check
after individual failures, then return one actionable end-of-run report for
all failures. Committed runtime/platform work then uses `deliver`.
Performance and diagnosis use the flag-free `benchmark` and `diagnose`
commands.

Native Linux CI owns Linux-specific Rust and Clippy assurance. Local planning
with `scripts/agent plan` shows the full affected operations without executing
them.
