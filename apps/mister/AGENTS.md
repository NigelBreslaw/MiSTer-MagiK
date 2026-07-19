# AGENTS.md - apps/mister

Root `AGENTS.md` applies, especially device and boot-loop safety.
Consult `docs/agents/file-authority.md` before editing generated or derived
files.

## Ownership

This directory owns the Rust/Slint device frontend, UI composition, and ARM
build. Portable input/domain state lives in `crates/magik-core`; framebuffer,
FPGA, VT, and device settings adapters live in `mister/platform/runtime`.

Start with `src/lib.rs` for host-testable logic, `src/main.rs` for command
dispatch, `src/ui_runner.rs` for UI entry, and `ui/launcher.slint` for the
production UI. Read `BUILD.md`, `docs/architecture.md`, and `docs/catalog.md`.

## Editing

- Hand-edit `src/`, `catalog/src/`, `ui/`, and `ui-generated` crate glue.
- Slint-generated Rust lives under Cargo `OUT_DIR`; never edit it.
- Keep portable logic in `crates/magik-core` and MiSTer hardware logic in the
  platform runtime.
- RGB565, cached-RAM rendering, and Main-mediated handoff are production rules.
- Experiments stay behind their existing features and are not production proof.

## Checks

```bash
scripts/dev-rust check
scripts/dev-rust test
scripts/validate paths apps/mister
```

Use the production UI check selected by `scripts/validate` for a host-local
Slint compile. ARM checks use the direct escalated command
`apps/mister/build-arm.sh --check --ui-scope launcher`; use `--ui-scope all
--experiments` only for all-scene/experiment changes.
