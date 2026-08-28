# AGENTS.md - MiSTer platform

This directory owns MiSTer-only hardware integration and operational tools.
`platform/kernel/` and `platform/fpga/` are qualified hardware sources;
`platform/contracts/` owns checked ABI representations; `platform/runtime/`
adapts those capabilities to portable interfaces. Do not leak file descriptors,
ioctls, physical addresses, or Main commands into portable code. Source moves
must preserve installed `/media/fat/mister-magik/**` paths and platform identity.
