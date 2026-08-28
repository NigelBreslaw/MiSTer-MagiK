# AGENTS.md - host scripts

Scripts own thin host validation, packaging, conversion, CI, release-data, and
analysis entrypoints. `scripts/magik-ci` is the Python implementation for CI
and release artifact processing; device, deployment, profiling, acceptance,
and recovery orchestration remains typed Rust.

- Use Bash with `set -euo pipefail` and preserve macOS Bash compatibility.
- Preserve stable command shapes used by sandbox approvals.
- Do not add device or workflow orchestrators. Agents use typed
  `scripts/agent deliver`, `benchmark`, or `diagnose`; attended human operations
  use `scripts/agent device`.
- Retry one explicitly read-only device request after transient transport
  failure. Never blindly replay mutation.
- Destructive runners require volatile arming and interruption-safe cleanup.
- Self-tests use temporary fixtures and never contact the MiSTer.
- Generated output belongs under ignored `build/`, `dist/`, `outputs/`, or a
  temporary directory.

`scripts/agent plan` previews assurance. Hooks and CI own automated checks;
agents do not reconstruct them.
