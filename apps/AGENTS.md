# AGENTS.md - applications

Root `AGENTS.md` applies. Applications compose portable domain crates with a
platform adapter and own their UI, binary entrypoints, and app-specific assets.

- `mister/` is the MiSTer device application; read its local `AGENTS.md`.
- `desktop/` is the macOS companion; read its local `AGENTS.md`.
- Shared domain and wire interfaces belong under `crates/`, not in an app.
- Future mobile applications are peers here; do not add mobile assumptions to
  `crates/magik-core` or MiSTer hardware assumptions to other apps.
