# Production zero-copy qualification result

## Outcome

True zero-copy is functionally correct with the coherent mailbox, but it does
not beat the production Home performance gate when all required ownership and
cache transfers are enabled. It must not become the production default on the
current implementation.

## Confirmed cause

The old path copies broad Home damage from cached RAM into the write-combined
hidden buffer. The atomic path eliminates that copy but replaces it with DMA
cache cleaning for the same broad region and hard PTE revocation. On this stock
Cyclone V kernel/device, those required operations cost more than the copy they
remove.

## Before

Production Home `home-repeat-hold` median gate:

- work p99: 6888 us
- hidden copy p99: 1479–1512 us across the three baseline samples

## Correct production candidate

`PROD-ZC-HOME-STEADY-1-20260711`, 10 seconds:

- work p99: 8052 us
- delta from baseline: +1164 us (+16.9%)
- latch frames: 565/565
- fallback, visual misses, alternation failures, FPGA errors: 0
- atomic post p99: 2140 us
- Slint render p99: 5775 us

## Falsification experiments

Removing hard PTE revocation improved work p99 to 7455 us, still 567 us slower
than baseline and not production-safe. The change was reverted.

Preserving Slint's individual `PhysicalRegion` rectangles while retaining hard
ownership produced work p99 8081 us. Home damage is already broad, so this did
not reduce cache-clean cost. The change was reverted.

## Slint 1.17 upstream review

The Slint monorepo at revision
`64a1241e4763e075b293b29d58d16c99f483d943` confirms that there is no
additional software-renderer damage signal for this integration:

- `SoftwareRenderer::render_buffer_impl` implements `SwappedBuffers` by
  repainting the union of the current frame's damage and `prev_frame_dirty`.
  That is the required two-buffer history, not an avoidable full-frame policy.
- `TargetPixelBuffer::line_slice` exposes a mutable scanline to the renderer,
  but it does not report the byte ranges actually written. Per-pixel
  instrumentation would add work below the already available
  `PhysicalRegion` signal.
- The STM32 double-buffer example calls `clean_dcache_by_slice(work_fb)` for
  the entire work buffer. It demonstrates the same cache-ownership transfer,
  not a cheaper range-cleaning mechanism.
- Slint's documented two-line-buffer DMA overlap belongs to push displays that
  transfer rendered scanlines to a controller. MiSTer's FPGA continuously
  scans a persistent framebuffer, so that approach would reintroduce a copy
  rather than provide zero-copy scanout.
- `cache-rendering-hint` is a renderer-dependent layer-cache hint. The Slint
  software renderer does not implement cached subtree layers, so applying it
  to the moving Home tiles cannot reduce this path.
- More than two scanout buffers would require damage since the older contents
  of each selected buffer. It increases the required damage history for this
  workload and does not reduce cache maintenance.

The review therefore leaves the measured result unchanged: Slint already
provides the useful renderer-level damage, and the remaining cost is the
Linux DMA ownership transfer plus hard mapping revocation for broad scrolling
damage.

Potential future work needs a materially different display or mapping design,
such as hardware address-offset scrolling with a ring-shaped scanout surface,
or a demonstrably cheaper kernel write-revocation mechanism. Either requires a
new before/after qualification and must be compared with the same optimization
on the legacy cached-RAM-to-WC path.

## Decision

- Keep the coherent mailbox, ownership API, diagnostics, and opt-in path.
- Do not flip production default.
- Do not weaken cache cleaning or hard ownership to manufacture a benchmark
  win.
- Reconsider only with a materially different mapping/revocation design or a
  workload whose real dirty ranges are sufficiently smaller.

Evidence:

- `history/2026-07-11-production-zero-copy-baseline.md`
- `build/launcher-home-scroll-profiles/PROD-ZC-HOME-STEADY-1-20260711-*`
- `build/launcher-home-scroll-profiles/PROD-ZC-NO-REVOKE-AB-20260711-*`
- `build/launcher-home-scroll-profiles/PROD-ZC-EXACT-DAMAGE-20260711-*`
