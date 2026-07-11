# True zero-copy fundamental limit — 2026-07-11

## Outcome

True zero-copy Slint scanout is functionally correct on MiSTer, but it is not a
performance win with the current stock-kernel ownership design. The fundamental
limit is that the Cortex-A9 write-back caches and the FPGA pixel-scanout path are
not cache-coherent.

In one sentence:

> On MiSTer, fast CPU-renderable memory and fast FPGA-visible pixel memory are
> not one coherent domain, and synchronizing a broadly dirty cached frame costs
> more than the existing write-combined copy.

The legacy latch therefore remains the production default. The atomic mailbox,
cacheable scanout allocation, ownership ABI, diagnostics, and opt-in path remain
useful infrastructure, but they do not justify enabling zero-copy by default.

## The memory-system tradeoff

Slint renders efficiently into normal write-back cacheable memory. CPU writes
can remain dirty in the Cortex-A9 L1 caches or the shared PL310 L2 cache. The
FPGA framebuffer reader, however, scans pixels directly from SDRAM through the
normal FPGA-to-SDRAM path and cannot observe those dirty cache lines.

A correct cacheable zero-copy present must therefore:

1. Clean every damaged CPU cache line to SDRAM.
2. Atomically transfer the slot from CPU ownership to FPGA ownership.
3. Revoke the userspace PTEs so no CPU writer can modify a queued or active
   slot.
4. Wait for the FPGA's vblank completion fence.
5. Return the slot to CPU ownership and fault its pages back into the process
   when it is reused.

The production implementation performs the range cache clean in
[`scanout_publish_locked`](../kernel/plugin-probe/mister_magik_plugin_probe.c)
and revokes the slot mapping with `unmap_mapping_range()` in
`MISTER_MAGIK_SCANOUT_SYNC_RANGES_AND_POST`.

The coherent ACP path does not change this pixel-memory contract. ACP carries
only the 4 KiB descriptor/completion mailbox; pixel buffers deliberately remain
ordinary software-managed DMA memory. See
[`docs/scanout-mailbox.md`](../docs/scanout-mailbox.md).

## Why the legacy copy wins

The legacy latch renders into cached heap RAM, then performs one sequential
copy into an inactive write-combined hidden framebuffer. The source is already
hot cacheable memory and the destination is an efficient streaming WC mapping.
On moving Home frames that copy costs approximately 1.5 ms at p99.

Zero-copy removes the explicit `memcpy`, but broad Home motion still requires
most of the changed pixel payload to be written from cache to SDRAM. On this
Cortex-A9 and stock MiSTer kernel, DMA cache maintenance is slower than the
optimized cached-to-WC streaming copy. Hard ownership then adds PTE invalidation,
TLB churn, and page faults on reuse.

The result is not that no bytes move. The movement has changed from an explicit,
efficient copy to a more expensive cache-coherency and ownership transfer.

## Production evidence

### Home

The matched `home-repeat-hold` qualification produced:

| Metric | Legacy latch | Correct atomic zero-copy |
|---|---:|---:|
| Work p99 | 6,888 us | 8,052 us |
| Hidden copy p99 | 1,479–1,512 us | Eliminated |
| Atomic post p99 | approximately 50 us | 2,140 us |
| Integrity violations | 0 | 0 |

The correct atomic path was 1,164 us, or 16.9%, slower overall.

Two falsification experiments separated the costs:

- Removing hard PTE revocation improved work p99 to 7,455 us, but remained
  567 us slower than the legacy baseline and was not production-safe. PTE
  revocation is therefore a substantial secondary cost, not the root cause.
- Preserving Slint's individual `PhysicalRegion` rectangles produced 8,081 us
  work p99. Home scrolling damage is already broad, so finer rectangle
  bookkeeping did not materially reduce cache-clean work.

See the authoritative
[`production zero-copy qualification`](2026-07-11-production-zero-copy-qualification.md)
and commit `47508e1`.

### Arcade

Arcade made the mismatch more visible:

| Metric | Legacy latch | Atomic zero-copy | Delta |
|---|---:|---:|---:|
| Corrected work p99 | 5,571 us | 12,269 us | +120.2% |
| Slint render p99 | 408 us | 476 us | +68 us |
| Hidden composition p99 | 1,974 us | 3,759 us | +1,785 us |
| Atomic/legacy post p99 | 50 us | 5,309 us | +5,259 us |

Slint itself barely changed. The preview and Arcade layers still had to be
blitted into the cacheable scanout slot, after which the resulting broad dirty
ranges were cache-cleaned. PTE revocation also caused the next writes to fault
the scanout pages back into the process.

See the
[`Arcade atomic zero-copy qualification`](2026-07-11-arcade-zero-copy-qualification.md)
and commit `1960d70`.

## Why the first proof appeared successful

The initial cacheable scanout proof reduced Home work p99 from 6,911 us to
5,471–5,540 us, a repeatable 19.8–20.8% improvement. That result correctly
proved that:

- A registered platform device could allocate cacheable, physically contiguous
  scanout slots on the stock kernel.
- Slint 1.17 could render directly into alternating cacheable slots using
  `RepaintBufferType::SwappedBuffers`.
- Dirty-range cache synchronization and the existing latch could display the
  result without visual or alternation errors in the proof configuration.

It deliberately excluded hard PTE revocation, atomic sync-and-post ownership,
the coherent completion mailbox, and direct Arcade/preview composition. It was
therefore a valuable but incomplete performance proof, not a production-safe
comparison. See
[`2026-07-11-true-zero-copy-slint-proof.md`](2026-07-11-true-zero-copy-slint-proof.md)
and commits `a37f369` and `c69f4af`.

## Why this was not fundamentally a Slint failure

Slint's `SwappedBuffers` policy redraws the union of current damage and the
previous frame's damage. That is required to keep alternating buffers correct;
it is not an accidental full-frame policy. The pinned Slint 1.17 review found
no more useful renderer-level write signal than the existing `PhysicalRegion`.

The alternatives also do not escape the MiSTer memory tradeoff:

- Rendering directly into WC memory avoids cache cleaning, but correct
  double-buffered Slint rendering into WC memory previously took 11–12 ms.
  Partial rendering, blending, and read-modify-write access are poor matches
  for WC memory. See
  [`2026-6-9/direct-framebuffer-sidecar-retrospective.md`](2026-6-9/direct-framebuffer-sidecar-retrospective.md).
- More scanout buffers require damage history extending back to the older
  contents of the selected buffer and do not make cache maintenance cheaper.
- Line-buffer DMA is appropriate for push displays. MiSTer's FPGA continuously
  scans a persistent framebuffer, so transferring rendered scanlines would
  reintroduce a copy rather than provide persistent zero-copy scanout.
- Routing bulk pixel scanout through ACP would be a materially different FPGA
  reader and memory architecture. The current design correctly limits ACP to
  small coherent control data.

## Architectural implication

Zero-copy can become attractive only if the amount of pixel memory crossing
the CPU/FPGA ownership boundary becomes much smaller, or if MiSTer gains a
demonstrably cheaper coherent mapping and revocation mechanism.

The most promising direction is therefore not another framebuffer mapping. It
is persistent hardware-visible surfaces with FPGA address-offset scrolling,
wraparound list planes, preview planes, and other scene-level composition. Those
features turn broad animation damage into small surface updates and descriptor
changes, reducing both explicit copies and cache-clean traffic.

Any future retry must retain full cache correctness and hard ownership, then
beat the same optimization applied to the legacy cached-RAM-to-WC path under
the existing Home, Arcade, and preview qualification gates.
