# Script layout

The `scripts/` tree is limited to the `scripts/agent` lifecycle entrypoint and
genuinely host-only packaging, conversion, CI, release-data, and pure-analysis
tools.

- `bench/analyze/` and `bench/reports/` — offline evidence analysis
- `checks/` — static architecture and workflow checks
- `media/` — host conversion and manifest generation
- `release/` — host release-data and packaging tools
- `tests/` — host-local contract tests

Device, ARM-build, deployment, profiling, acceptance, recovery, and scene
orchestration belongs in Rust. New shell interfaces in those categories are
rejected by `scripts/checks/check-no-operational-shell-orchestrators.py`.

Normal repository work uses bounded Rust analyzer diagnostics where applicable,
explicit-path `git add`, ordinary `git commit`, and `git push`; the pre-commit
hook runs the bootstrap-free Python fast gate under its ten-second deadline.
The pre-push hook enters `agent-cli` for full affected assurance before
committed work reaches the remote. Committed runtime/platform work then uses
`deliver`.
Performance and diagnosis use the flag-free `benchmark` and `diagnose`
commands.

Native Linux CI owns Linux-specific Rust and Clippy assurance. Local planning
with `scripts/agent plan` shows the full affected operations without executing
them.
