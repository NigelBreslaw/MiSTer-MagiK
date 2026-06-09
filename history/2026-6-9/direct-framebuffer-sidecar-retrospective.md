# Direct Framebuffer Sidecar Retrospective

Date: 2026-06-09

## Decision

Stop pursuing the direct framebuffer / fbwc sidecar path.

The production direction is the cached `/dev/fb0` renderer with focused
optimization work there. A follow-up PR should remove the kernel module,
`fbwc-*` commands, direct framebuffer render modes, and associated cleanup
surface.

## What was tried

- `fbwc-direct`: Slint renders directly into a write-combined hidden framebuffer
  exposed by the sidecar module, then HDMI is routed to that buffer.
- `fbwc-double`: sidecar module exposes buffers 1 and 2; Slint renders into the
  non-visible WC buffer and flips after render.
- `fbwc-shadow`: Slint renders into cached RAM, dirty rectangles are mirrored
  into two WC buffers, then HDMI flips between buffers after the full frame
  update.

The sidecar module was extended from one mapped hidden buffer to two hidden
buffers and reports `version=2`, `buffer_count=2`.

## Validation results

The non-Slint `fbwc-flip-test` looked visually stable, including a half-solid
screen pattern intended to expose flicker.

Real Slint UI did not pass:

- `fbwc-direct` visibly flickered on the home/system page and arcade list.
- `fbwc-double` removed some direct-write artifacts but initially lost arcade
  list text until the frame preparation bug was fixed.
- `fbwc-shadow` worked functionally but still showed visual glitches in real
  use.

## Benchmarks

Important metric: not merely whether a mode reaches 60fps, but how much CPU
time it burns before vsync. Cached mode already reaches 60fps; the goal was
more headroom for richer visual effects.

Clean scene benchmark summary:

- `cached` broad scenes: `slint-render ~0.9ms`, `fb-present ~0.7ms`.
- `fbwc-direct` broad scenes: `slint-render ~0.75ms`, `fb-present ~0.001ms`,
  but visually flickers.
- `fbwc-double` broad scenes: `slint-render ~11-12ms`, `fb-present ~0.27-0.30ms`.
- `fbwc-shadow` broad scenes: `slint-render ~0.9-1.0ms`,
  `fb-present ~0.95-1.1ms`.

Focused launcher held-scroll benchmark:

- `cached`: active arcade-list frames were roughly `4-6ms` busy
  (`prepare + slint-render + custom-draw + fb-present`).
- `fbwc-double`: roughly `17-18ms` busy; too slow.
- `fbwc-shadow`: roughly `7-9ms` busy; better than double, but still worse than
  cached and visually glitchy.

## Interpretation

The sidecar proves WC hidden-buffer writes can be fast, but real Slint and the
custom arcade-list overlay need careful consistency across partial updates.
Once correctness is restored with double buffering or shadow mirroring, the CPU
cost erodes the reason to leave the cached path.

The cached path is simpler, update_all-safe, kernel-module-free, visually
stable, and currently has the best CPU headroom for the launcher.

## Follow-up PR

Remove the experimental direct framebuffer stack:

- `kernel/fbwc/`
- fbwc module build/deploy artifacts and scripts
- `fbwc-probe`, `fbwc-bench`, `fbwc-flip-test`
- `MISTER_UI_RENDER_MODE=fbwc-*`
- `UiFrameTarget` direct/shadow variants
- sidecar load/unload lifecycle hooks
- docs that describe direct framebuffer as an active path

Then focus optimization work on cached rendering and the arcade-list overlay.
