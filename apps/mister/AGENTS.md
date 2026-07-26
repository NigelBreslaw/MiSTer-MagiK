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

- Hand-edit `src/`, `ui/`, and `ui-generated` crate glue. Catalog Rust lives in
  `crates/catalog/src/`.
- Slint-generated Rust lives under Cargo `OUT_DIR`; never edit it.
- Keep portable logic in `crates/magik-core` and MiSTer hardware logic in the
  platform runtime.
- RGB565, cached-RAM rendering, and Main-mediated handoff are production rules.
- Experiments stay behind their existing features and are not production proof.

## Assurance

Use `$magik-rust-lsp` for Rust navigation and diagnostics while editing, and
the Slint MCP for UI behavior. The pre-commit hook checks staged formatting and
policy; pre-push and CI select the production UI, tests, Clippy, and ARM
assurance required by the changed files. `scripts/agent plan` previews that
full affected plan without executing it.
