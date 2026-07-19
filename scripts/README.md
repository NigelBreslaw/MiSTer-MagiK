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
