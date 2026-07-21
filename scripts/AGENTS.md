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

Run `scripts/agent check` to inspect task-scoped local prerequisites without
contacting the MiSTer.

## Rules

- Use Bash with `set -euo pipefail`; keep macOS Bash compatibility.
- Preserve stable direct command shapes used by sandbox approvals.
- Do not add device/build/deploy/profile/acceptance shell orchestrators.
- Human device operations use the typed Rust `mister` host binary; agents use
  `scripts/agent deliver`, `benchmark`, or `diagnose`.
- Device/network commands must fail after one bounded wrapper attempt.
- Destructive runners require interruption-safe cleanup and volatile arming.
- Self-tests must use temporary/local fixtures and never contact the MiSTer.
- Generated output belongs under ignored `build/`, `dist/`, or a temporary
  directory unless explicitly curated evidence.

## Checks

```bash
bash -n scripts/NAME.sh
scripts/agent check
scripts/agent verify
```

The task baseline supplies the affected paths. `--paths` is reserved for CI and
diagnostics.
