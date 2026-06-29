# Framebuffer experiments — getting Slint to a locked, tear-free 60fps

Goal: drive Slint on the MiSTer's `/dev/fb0` (1920×1080×32, BGRX) at a steady
60fps, smooth and tear-free. The panel is 60Hz and
`MiSTer_fb` supports `FBIO_WAITFORVSYNC` (`0x40044620`, measured rock-steady
16.6ms = 60Hz). Below is each approach we tried, its measured per-frame cost,
and what it taught us.

## The experiments

| # | Approach | Render | Present/flip | Result | Tear |
|---|----------|--------|--------------|--------|------|
| 1 | **Python/Slint linuxfb** (built-in backend) | full-frame SW | no vsync, blits immediately | ~62fps, **~96% of one A9 core**, drifts vs 60Hz | ✗ tears |
| 2 | **Rust: cached render + full blit** to `/dev/fb0` after vsync | 2.3ms (cached) | full 8MB memcpy **11.5–13.6ms** | ~57fps, **juddery** (present creeps over budget → drops) | mostly ok |
| 3 | **Rust: FPGA page-flip**, render *direct* into `/dev/mem` back buffers (`SwappedBuffers`) | **15–17.7ms** (uncached, current+prev dirty) | flip **~12µs** | locked 61 in steady state but **unstable** — collapsed to 30fps for ~10s stretches when dirty area grew (render > 16.6ms) | ✓ tear-free |
| 4 | **Rust: cached render + dirty-row copy** into `/dev/mem` back buffers, then flip | 2.3ms (cached) | copy 620 rows = **45ms** (~105MB/s) | **~20fps** | ✓ tear-free |
| 5 | ✅ **Rust: cached render + dirty-row copy** into single `/dev/fb0` buffer after vsync | 2.3ms (cached) | copy 619 rows **8.7ms** (~546MB/s) | **locked 59.9–61fps, zero drops over 22s**, ~5.6ms slack | ✓ tear-free in practice |

Winner is #5. Per-frame budget:

```
render 2.3ms (cached RAM)  +  vsync-wait ~5.6ms  +  dirty-row copy ~8.7ms  ≈ 16.6ms
```

## The key finding: write-combining vs uncached

This is what made #4 fail and #5 win, and why true page-flipping is a dead end
on this hardware:

- **`/dev/fb0`'s driver mmap is write-combining (~700MB/s).** Fast.
- **mmapping the *same* physical buffers via `/dev/mem`** (as MiSTer's
  `shmem_map` does, `O_RDWR|O_SYNC`) **is uncached device memory (~105MB/s)** —
  about **7× slower**. Confirmed: 620 full-width rows = 4.75MB took 45ms via
  `/dev/mem` (#4) but 8.7ms via `/dev/fb0` (#5).
- **`/dev/fb0` only ever exposes ONE buffer** (`virtual_size` = 1920×1080,
  `stride` = 7680). So there is no *second* write-combining buffer to flip
  between.
- The FPGA flip itself is genuinely cheap and tear-free (`SET_FBUF` SPI,
  **~12µs**), and the back buffers (1/2) are valid reserved DDR — but they're
  only reachable through the slow `/dev/mem` map, so writing them can't keep up
  with 60fps for any non-trivial dirty area. Page-flipping is therefore off the
  table here.

## How #5 stays tear-free without a second buffer

Single buffer, but the copy is started **right after the vblank** and is faster
than the scan beam:

- Copy 619 rows at ~546MB/s ≈ **8.7ms**.
- The beam needs 619/1080 × 16.6 ≈ **9.5ms** to traverse the same rows.
- So the row-by-row copy stays just ahead of the beam → no visible tear.

## Supporting facts established along the way

- **Memory map is safe to touch:** `/proc/iomem` shows System RAM ends at
  `0x1FEFFFFF`; everything from `0x20000000` up (FB at `0x22000000`) is reserved
  FPGA DDR, not kernel RAM. So `/dev/mem` writes to buffers 1/2 don't corrupt
  Linux — the limitation is purely speed, not safety.
- **Render is cheap when cached:** Slint's `SoftwareRenderer` with
  `RepaintBufferType::ReusedBuffer` redraws only the dirty region into a cached
  `Vec` in ~2.3ms; `render()` returns a `PhysicalRegion` whose bounding-box rows
  are exactly what we copy.
- **Pixel format:** framebuffer is BGRX, but writing `0x00RRGGBB` gives correct
  colours on HDMI (verified) — no manual R/B swap needed in the `TargetPixel`
  impl.
- **We own the loop:** a custom Slint `Platform` (one `MinimalSoftwareWindow`,
  monotonic clock) lets us pace on `FBIO_WAITFORVSYNC` instead of calling
  `run_event_loop`. This is the piece the Python bindings don't expose, which is
  why the locked-60 path is Rust-only.

## Remaining headroom / follow-ups

- The demo copies ~half the screen only because its gradient bar spans the full
  width (full-width dirty rows). A real UI with localised motion copies far less.
- Could copy the dirty *sub-rectangle* (x-range) rather than full-width rows for
  further savings.
- `xoff/yoff` are hardcoded `0,0` for this 1080p `direct_video` menu; deriving
  them from the live mode (`UIO_GET_VRES` + timing) would generalise to other
  resolutions / CRT.
