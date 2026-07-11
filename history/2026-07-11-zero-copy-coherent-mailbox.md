# Zero-copy coherent DMA mailbox

## Confirmed cause

The descriptor/completion mailbox used an ordinary cached page without a DMA
allocation contract. FPGA ACP reads could observe CPU descriptor writes, but
the CPU repeatedly read four zero completion words after the FPGA had applied
the same sequence. Ad hoc cache maintenance only changed how many frames
completed before the stall.

The mailbox is now allocated and freed with `dma_alloc_coherent()` and
`dma_free_coherent()` on the scanout platform device. Pixel buffers remain
cacheable non-coherent DMA allocations with range ownership transfers.

## Before

Gate `PROD-ZC-FENCE-DEBUG-20260711`:

- completion words observed by CPU: `00000000/00000000/00000000/00000000`
- pending sequence: 1
- measured latch frames: 0
- measured backend: 100% `fb0-dirty` after fallback

## After

Gate `PROD-ZC-COHERENT-MAILBOX-20260711`:

- latch frames: 84/84
- fallback frames: 0
- visual latch misses: 0
- buffer alternation failures: 0
- minimum latch margin: 6546 us

The gate's overall invalid result is a separate harness issue: it still
requires legacy latch counters even though mailbox backend validation passed.

Evidence:

- `build/launcher-home-scroll-profiles/PROD-ZC-FENCE-DEBUG-20260711-launcher-home-scroll.log`
- `build/launcher-home-scroll-profiles/PROD-ZC-COHERENT-MAILBOX-20260711-launcher-home-scroll.tsv`
- `build/launcher-home-scroll-profiles/PROD-ZC-COHERENT-MAILBOX-20260711-fpga-latch-after.log`

Validation:

- `scripts/build-plugin-probe-module.sh`
- Stock MiSTer 5.15 module load
- Two-second real-device Home maximum-scroll gate
