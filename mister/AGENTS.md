# AGENTS.md - MiSTer platform

Root `AGENTS.md` applies, including the critical boot-loop safety rules. This
directory owns MiSTer-only hardware integration and operational tools.

- `platform/kernel/` and `platform/fpga/` are qualified hardware sources.
- `platform/contracts/` owns checked Rust representations of hardware ABIs.
- `platform/runtime/` adapts MiSTer capabilities to portable domain interfaces;
  do not leak file descriptors, ioctls, physical addresses, or Main commands.
- `tools/agent/` and `tools/host/` retain their local safety instructions.
- Source relocation must never rename installed `/media/fat/mister-magik/**`
  paths or weaken platform-contract identity checks.
