# AGENTS.md - scripts

Root `AGENTS.md` applies.
File authority and regeneration commands are indexed in
`docs/agents/file-authority.md`.

## Ownership

Scripts provide host validation, packaging, conversion, CI, release-data, and
pure-analysis interfaces. Device, build, deployment, profiling, acceptance,
and recovery orchestration belongs in typed Rust.

Stable public commands remain directly under `scripts/`. Shared implementation,
checks, tests, analysis, media, and release helpers are organized according to
`scripts/README.md`.

Use `scripts/agent plan` to preview affected assurance without executing it or
contacting the MiSTer.

## Rules

- Use Bash with `set -euo pipefail`; keep macOS Bash compatibility.
- Preserve stable direct command shapes used by sandbox approvals.
- Do not add device/build/deploy/profile/acceptance shell orchestrators.
- Human device operations use the typed Rust `mister` host binary; agents use
  `scripts/agent deliver`, `benchmark`, or `diagnose`.
- Local Main experiments use only the positional
  `scripts/agent deliver local-main` workflow. Do not add feature-flag or shell
  deployment alternatives.
- Explicitly read-only device requests may retry once after transient transport
  unavailability. Mutating requests must not be replayed outside their typed
  reconciliation or compensation path.
- Destructive runners require interruption-safe cleanup and volatile arming.
- Self-tests must use temporary/local fixtures and never contact the MiSTer.
- Generated output belongs under ignored `build/`, `dist/`, or a temporary
  directory unless explicitly curated evidence.

## Assurance

Stage intentional script changes and commit them normally. Pre-commit performs
bounded shell syntax, whitespace, and policy checks. Pre-push and CI run the
affected script contracts, packaging fixtures, and other full assurance. Agents
do not construct those checks directly.
