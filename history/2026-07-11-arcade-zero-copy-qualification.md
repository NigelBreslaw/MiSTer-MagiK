# Arcade atomic zero-copy qualification

## Outcome

The Arcade view runs correctly through the atomic cacheable scanout path after
the launcher guarantees the second `SwappedBuffers` render and makes the latch
plan follow the scanout slot actually acquired by the renderer. It is much
slower than the legacy latch and must remain opt-in.

Both accepted traces use the same 30-second `turbo-hold` workload, 960x540
RGB565 output, `fpga-vblank-latch-hidden`, catalog refresh off, and framebuffer
streaming off.

## Correct work accounting

The existing Arcade analyzers defined work as `prepare + Slint + custom draw +
fb-present`. That omitted `hidden_compose_us`, even though preview and Arcade
layer composition runs inside the latch present transaction. The comparison
below uses:

`prepare_us + slint_render_us + custom_draw_us + hidden_compose_us + fb_present_us`

The analyzers now use this formula. The old reported work values are retained
below only to make the accounting correction auditable.

## Matched device results

| metric | Legacy latch | Atomic zero-copy | Delta |
|---|---:|---:|---:|
| measured frames | 1,800 | 1,796 | -4 |
| corrected work p50 | 4,233 us | 8,804 us | +4,571 us |
| corrected work p99 | 5,571 us | 12,269 us | +6,698 us (+120.2%) |
| old reported work p99 | 4,024 us | 9,007 us | +4,983 us |
| Slint render p99 | 408 us | 476 us | +68 us |
| hidden composition p99 | 1,974 us | 3,759 us | +1,785 us |
| preview composition p99 | 660 us | 1,456 us | +796 us |
| Arcade composition p99 | 1,446 us | 2,581 us | +1,135 us |
| fb-present p99 | 111 us | 5,341 us | +5,230 us |
| atomic/legacy post p99 | 50 us | 5,309 us | +5,259 us |
| wall p99 | 16,835 us | 17,087 us | +252 us |

The atomic trace has zero fallback, timeout, error, latch deadline miss,
buffer-alternation failure, FPGA drop, or mailbox error. The mailbox applied
2,730 descriptors during the measurement window.

## Correctness findings needed to obtain the atomic trace

The first device attempt rendered into scanout slot 1 while the legacy latch
planner independently selected slot 2. Making the atomic plan follow the
acquired renderer slot exposed the underlying second-frame condition: after
the first post, Slint had no new base-layer damage and did not invoke the
render callback, so the cached frame still named the now-active slot.

`RepaintBufferType::SwappedBuffers` requires the other target to be visited
after a post so the renderer can replay the previous damage. The launcher now
requests that follow-up render while atomic latch scanout is active. The latch
still rejects any acquired slot that its synchronized hardware state does not
consider writable.

## Decision

- Keep legacy latch as the production default.
- Retain the atomic Arcade correctness fixes and explicit benchmark selector.
- Do not describe the custom Arcade/preview path as copy-free: it blits cached
  direct layers into the cacheable scanout slot before the atomic cache clean.
- Any future comparison must include `hidden_compose_us` in work time.

Evidence:

- `build/arcade-scroll-profiles/ZC-ARCADE-LEGACY-20260711-*`
- `build/arcade-scroll-profiles/ZC-ARCADE-ATOMIC-R3-20260711-*`
- Invalid diagnostic attempts:
  `build/arcade-scroll-profiles/ZC-ARCADE-ATOMIC-20260711-*` and
  `build/arcade-scroll-profiles/ZC-ARCADE-ATOMIC-R2-20260711-*`
