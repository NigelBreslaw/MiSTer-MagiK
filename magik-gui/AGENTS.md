# AGENTS.md - magik-gui

Root `AGENTS.md` applies, especially device and boot-loop safety.

## Ownership

This directory owns the Rust/Slint device frontend, catalog path crate,
framebuffer control, input/navigation logic, and ARM build.

Start with `src/lib.rs` for host-testable logic, `src/main.rs` for command
dispatch, `src/ui_runner.rs` for UI entry, and `ui/launcher.slint` for the
production UI. Read `BUILD.md`, `docs/architecture.md`, and `docs/catalog.md`.

## Editing

- Hand-edit `src/`, `catalog/src/`, `ui/`, and `ui-generated` crate glue.
- Slint-generated Rust lives under Cargo `OUT_DIR`; never edit it.
- Keep portable logic out of Linux-only input/framebuffer modules.
- RGB565, cached-RAM rendering, and Main-mediated handoff are production rules.
- Experiments stay behind their existing features and are not production proof.

## Checks

```bash
scripts/dev-rust check
scripts/dev-rust test
scripts/validate paths magik-gui
```

Use `scripts/dev-rust check-ui` for production Slint work and
`check-ui-full` only for all-scene/experiment changes. ARM builds and device
commands require escalation; ordinary host checks do not contact the MiSTer.
