# True zero-copy Slint scanout proof — 2026-07-11

Status: experimental branch only; opt-in with `MISTER_TRUE_ZERO_COPY=1`.

## Confirmed cause

The legacy latch path renders into cacheable heap RAM and copies damage into a
write-combined hidden framebuffer. On the matched Home motion gate that copy
costs 1,265 us median and 1,496 us p99. The earlier `dma_alloc_wc` failure was
specific to the bare misc-device allocation: a registered platform device plus
`dma_alloc_noncoherent(..., DMA_TO_DEVICE)` successfully allocated two
cacheable, physically contiguous 1,843,200-byte scanout slots on the stock
5.15.1-MiSTer kernel.

The proof maps those slots write-back only, asks Slint 1.17.0 to use
`RepaintBufferType::SwappedBuffers`, renders directly into alternating slots,
and cleans only the planned dirty ranges before the existing 0x57 vblank post.
It never creates a write-combined alias for the plugin-owned pages.

## Matched device evidence

All three runs use the real launcher Home `home-repeat-hold` scenario for 30s,
960x540 RGB565, and `fpga-vblank-latch-hidden`.

| metric | BEFORE | AFTER R1 | AFTER R2 |
|---|---:|---:|---:|
| valid frames | 1,765 | 1,764 | 1,764 |
| latch copy p50 | 1,265 us | 1 us | 1 us |
| latch copy p99 | 1,496 us | 2 us | 2 us |
| total work p99 | 6,911 us | 5,540 us | 5,471 us |
| total work max | 8,698 us | 6,624 us | 6,570 us |
| latch margin min | 7,903 us | 9,983 us | 10,027 us |
| deadline / visual / alternation / FPGA drops | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 |

Repeatable work-p99 saving: 1,371–1,440 us, or 19.8–20.8% versus BEFORE.

Artifacts:

- BEFORE: `/Users/nigelb/slint/mister-slint/build/launcher-home-scroll-profiles/ZEROCOPY-HOME-BEFORE-20260711-R1-*`
- AFTER R1: `/private/tmp/mister-slint-true-zero-copy/build/launcher-home-scroll-profiles/ZEROCOPY-HOME-AFTER-20260711-R1-*`
- AFTER R2: `/private/tmp/mister-slint-true-zero-copy/build/launcher-home-scroll-profiles/ZEROCOPY-HOME-AFTER-20260711-R2-*`

## Scope still deliberately excluded

This proves the performance premise and cacheable allocation path. It does not
yet make the path production-default. Hard PTE write revocation, atomic
`SYNC_RANGES_AND_POST`, ACP descriptor transport, and direct Arcade/preview
layer composition remain required before production enablement. The current
opt-in path fails safely to the legacy cached/WC-copy path when the scanout
device is unavailable.
