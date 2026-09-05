# MiSTer frontend

Start with `src/lib.rs` for host-testable logic, `src/main.rs` for dispatch,
`src/ui_runner.rs` for runtime composition, and `ui/launcher.slint` for UI.
Hardware adapters belong in `mister/platform/runtime`.

Hand-edit `src/`, `ui/`, and `ui-generated` glue, never Cargo `OUT_DIR`.
Keep experiments behind existing features; they are not production proof.

When source/tests are insufficient, consult only the relevant section of
`BUILD.md`, `docs/architecture.md`, or `docs/catalog.md`. Physical device evidence
is required for hardware-dependent claims, not ordinary portable changes.
