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

Normal repository work uses `scripts/agent task begin`, `check`, `verify`, and
`commit`; committed runtime/platform work then uses `deliver`. Performance and
diagnosis use the flag-free `benchmark` and `diagnose` commands.
