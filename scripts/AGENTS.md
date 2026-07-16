# AGENTS.md - scripts

Root `AGENTS.md` applies.
File authority and regeneration commands are indexed in
`docs/agents/file-authority.md`.

## Ownership

Scripts provide the approved validation, build, deploy, device, packaging,
profiling, and release interfaces. Prefer extending an existing entrypoint over
creating a near-duplicate command.

Stable public commands remain directly under `scripts/`. Shared implementation,
checks, tests, analysis, media, and release helpers are organized according to
`scripts/README.md`.

Run `scripts/doctor --scope full-host` to inspect local prerequisites without
contacting the MiSTer.

## Rules

- Use Bash with `set -euo pipefail`; keep macOS Bash compatibility.
- Preserve stable direct command shapes used by sandbox approvals.
- Use `scripts/mister`, never raw SSH/SCP.
- Device/network commands must fail after one bounded wrapper attempt.
- Destructive runners require interruption-safe cleanup and volatile arming.
- Self-tests must use temporary/local fixtures and never contact the MiSTer.
- Generated output belongs under ignored `build/`, `dist/`, or a temporary
  directory unless explicitly curated evidence.

## Checks

```bash
bash -n scripts/NAME.sh
scripts/test-host-tools.sh --fast
scripts/validate paths scripts/NAME
```

Use `scripts/test-host-tools.sh --full` for packaging, release, installer, or
cross-script contract changes.
