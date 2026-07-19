# AGENTS.md - shared crates

Root `AGENTS.md` applies. This directory owns portable domain and protocol
crates shared by applications and platform adapters.

- Keep `magik-core` free of Slint, OS, filesystem, FPGA, process-control, and
  installed-layout knowledge.
- Wire crates preserve their existing schemas, bounds checks, and error modes.
- Platform hardware contracts belong under `mister/platform/contracts/`.
- Validate the narrow crate first, then use `scripts/validate paths PATH`.
