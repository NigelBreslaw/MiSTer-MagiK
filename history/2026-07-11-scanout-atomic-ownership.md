# Atomic scanout ownership evidence — 2026-07-11

## Confirmed cause

ABI v1 let userspace call `SYNC_DEVICE` and later send UIO command `0x57`.
Mappings remained writable between and after those calls, so the DMA ownership
transfer, route publication, and CPU write exclusion were advisory rather than
one kernel-enforced transaction.

## Before / after

- Before: 3 scanout ioctls, 0 hard-ownership capabilities, 0 atomic post
  operations, and 0 FPGA completion fences consumed by the plugin.
- After: 7 scanout ioctls (the 3 ABI-v1 compatibility calls plus 4 ABI-v2
  calls), 4 explicit capabilities, 1 `SYNC_RANGES_AND_POST` transaction, and 1
  coherent completion sequence controlling slot release.
- A device-owned slot now has 0 fault-insertable user pages. PTEs already
  present at post time are invalidated across the character-device mapping;
  later faults return `SIGBUS` until the completion fence releases the slot.
- Performance p99 remains the production baseline until the launcher switches
  to ABI v2: Home 6,888 us; Arcade 3,736 us; preview 2,469 us.

## Tests

- `scripts/build-plugin-probe-module.sh` compiled and linked
  `mister_magik_scanout.ko` for ARMv7 against stock `5.15.1-MiSTer` after the
  ABI, fault handler, DMA sync, and mailbox publication changes.
- `git diff --check`
- Repository pre-commit tests and clippy.

Real completion/fence and PTE-fault behavior is part of the final device
qualification gate; this commit deliberately does not claim an on-device
latency result before the matching RBF and launcher are deployed together.

## Evidence artifacts

- `kernel/plugin-probe/mister_magik_scanout_uapi.h`
- `kernel/plugin-probe/mister_magik_plugin_probe.c`
- `docs/scanout-mailbox.md`
- `build/plugin-probe/mister_magik_scanout.ko`
- `build/plugin-probe/modinfo.txt`
- `history/2026-07-11-production-zero-copy-baseline.md`
