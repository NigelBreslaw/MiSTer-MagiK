# AGENTS.md - MiSTer frontend

## Ownership

This directory owns the Rust/Slint device frontend and ARM composition. Start
with `src/lib.rs` for host-testable logic, `src/main.rs` for dispatch,
`src/ui_runner.rs` for UI entry, and `ui/launcher.slint` for production UI.
Portable state belongs in `crates/magik-core`; hardware adapters belong in
`mister/platform/runtime`.

Consult one canonical section only when source and tests do not answer the
question: `apps/mister/BUILD.md` for ARM/build policy, the relevant heading in
`docs/architecture.md` for lifecycle or presentation ordering, or the relevant
heading in `docs/catalog.md` for discovery and previews.

## Rules

- Hand-edit `src/`, `ui/`, and `ui-generated` glue. Never edit Cargo `OUT_DIR`.
- Keep portable logic free of MiSTer hardware details.
- RGB565, cached-RAM rendering, and Main-mediated handoff are production rules.
- Keep experiments behind existing features; they are not production proof.

Use `$magik-rust-lsp` for Rust and the Slint MCP for UI behavior. Device checks
are required only for hardware-dependent claims; hooks and CI own automated
assurance.
