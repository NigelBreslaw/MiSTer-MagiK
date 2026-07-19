# AGENTS.md - launcher runtime

Root and `apps/mister/AGENTS.md` apply.
File authority is documented in `docs/agents/file-authority.md`.

## Ownership

This directory owns launcher startup, scheduling, lifecycle, catalog/media
workers, presentation, composition, Slint bridging, and benchmark scenarios.
Start with `launcher_loop.rs`, then follow the named session/lifecycle module
for the behavior being changed. Use `docs/architecture.md` state charts as the
ordering source of truth.

## Rules

- Preserve lifecycle, composition, and display ownership invariants.
- Do not perform cache creation, network work, or blocking persistence on the
  UI hot path.
- Keep benchmark-only policy isolated from production defaults.
- A full Slint present invalidates direct Arcade layers; repaint them in the
  same frame when Arcade remains active.
- Tests should exercise event/state sequences, not only helper predicates.

## Checks

```bash
scripts/dev-rust test
scripts/dev-rust check
scripts/dev-rust check-ui
scripts/validate paths apps/mister/src/ui_runner
```

Device validation is required only when behavior depends on scan-out, timing,
input hardware, or Main handoff.
