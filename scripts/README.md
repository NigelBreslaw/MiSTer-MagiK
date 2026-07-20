# Script layout

Stable human, automation, and device entrypoints remain directly under
`scripts/`. Internal implementation is grouped by responsibility:

- `bench/analyze/` — trace and benchmark analysis
- `bench/reports/` — frame-profile report generation
- `checks/` — static architecture, release, and workflow checks
- `device/diagnostics/` — device evidence summarizers
- `lib/` — shared shell/Python implementation and fixtures
- `media/` — media conversion and manifest harvesting
- `release/databases/` — game-database release tooling
- `release/packaging/` — distribution metadata and legal inventory generation
- `release/platform/` — Main/FPGA/kernel identity, durable recovery, bundle, and manifest tools
- `tests/` — host-local tests for scripts and workflow contracts
- `experiments/` — non-production effect and preview experiments

Keep a command at the root when it is a documented public interface, a stable
sandbox approval shape, or an attended device workflow. Prefer adding reusable
implementation to the appropriate folder instead of adding another root-level
helper.

## Local workflow benchmarking

Use `scripts/bench-local-workflow.sh --tier quick --group all` to measure Git,
affected validation, ARM build, and simulated deploy iteration costs without
changing the real index or contacting a MiSTer. Use `--tier full` for five
measured Git/hook samples and two measured build/deploy samples, `--cold` for a
benchmark-owned cold ARM target, and `--device` only for an attended hardware
deploy sample. `--samples` and `--warmups` provide explicit bounded overrides.
Results are retained under the ignored `build/local-workflow-bench/` directory.
