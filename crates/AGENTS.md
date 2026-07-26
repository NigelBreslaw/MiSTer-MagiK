# AGENTS.md - shared crates

Root `AGENTS.md` applies. This directory owns portable domain and protocol
crates shared by applications and platform adapters.

- Keep `magik-core` free of Slint, OS, filesystem, FPGA, process-control, and
  installed-layout knowledge.
- Wire crates preserve their existing schemas, bounds checks, and error modes.
- Platform hardware contracts belong under `mister/platform/contracts/`.
- Use `$magik-rust-lsp` for Rust navigation and diagnostics while editing.
  Pre-commit checks staged formatting; pre-push and CI run affected tests,
  feature combinations, and full Clippy assurance.
