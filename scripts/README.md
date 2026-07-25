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

Normal repository work uses `scripts/agent check`, explicit-path `git add`, and
ordinary `git commit`; the pre-commit hook runs the bounded fast gate.
The pre-push hook performs full affected verification before committed work
reaches the remote. Committed runtime/platform work then uses `deliver`.
Performance and diagnosis use the flag-free `benchmark` and `diagnose`
commands.

Linux-only Rust and Clippy diagnostics can be reproduced from Apple Silicon
with:

```bash
scripts/agent-linux-verify --paths mister/tools/host mister/tools/agent
```

The command runs the normal verification harness inside the repository Apple
Linux image. Its Rust 1.97.1, Clippy, Rustfmt, Cargo, and target caches live
under `/private/tmp/mister-magik-linux-verify` by default. Apple container
execution requires first-attempt escalation.
