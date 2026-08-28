# AGENTS.md - shared crates

Shared crates own portable domain and protocol code. Keep `magik-core` free of
Slint, OS, filesystem, FPGA, process-control, and installed-layout knowledge.
Wire crates preserve schemas, bounds checks, and error modes. Hardware ABI
contracts belong under `mister/platform/contracts/`. Use `$magik-rust-lsp` for
edit-time feedback; hooks and CI own automated assurance.
