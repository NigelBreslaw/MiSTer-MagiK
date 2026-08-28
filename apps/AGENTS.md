# AGENTS.md - applications

Applications compose portable crates with a platform adapter and own their UI,
entrypoints, and app-specific assets. Shared domain and wire interfaces belong
under `crates/`. Keep MiSTer hardware assumptions out of desktop, mobile, and
portable code.
