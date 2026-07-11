# Zero-copy PFN mmap fix

## Confirmed cause

The scanout slots come from high-order DMA allocations. Tail pages do not have
the independent page references required by `vmf_insert_page()`, so faults after
the first mapped page returned `SIGBUS`. These physically contiguous mappings
must be exposed as PFNs with `VM_PFNMAP` and `vmf_insert_pfn()`.

## Before

- First scanout page: readable and writable.
- Second scanout page: process terminated with `SIGBUS`.
- Usable zero-copy frames: 0.

## After

- Both complete RGB565 scanout slots map successfully.
- Corrected RBF/device smoke reached 34 real
  `fpga-vblank-latch-hidden` frames with zero buffer-alternation failures.
- The subsequent fence-lifecycle failure is tracked separately and does not
  invalidate the mmap result.

Evidence:

- `build/launcher-home-scroll-profiles/PROD-ZC-ACP-FIX-20260711-launcher-home-scroll.tsv`
- `build/launcher-home-scroll-profiles/PROD-ZC-ACP-FIX-20260711-launcher-home-scroll.log`

Validation:

- `scripts/build-plugin-probe-module.sh`
- Stock MiSTer 5.15 module load and two-slot device smoke.
