# Receipt-scoped Arcade modal carrier — 2026-08-22

## Authority

- Experiment revision: `0e5794438c5594e3e3c8d0ee3a251b8f3b9d3ce3`
- Main revision: `639d3694e1b93660020e9587cd0fe27f0170ce4c`
- Display: `hdmi-1280x720p60`
- Performance authority: unprofiled installed Dev runtime
- Control: `build/agent-benchmarks/settled-composition/1787371706/summary.json`
- Candidate: `build/agent-benchmarks/settled-composition-receipt-scoped/1787371729/summary.json`

## Result

The control forced and copied every retirement-confirmed modal frame. The
candidate forced a carrier on entry, retained it through physical direct-layer
retirement, then stopped after the matching modal generation and receipt.

| Metric | Control | Candidate | Delta |
| --- | ---: | ---: | ---: |
| Steady frames | 52 | 51 | one candidate convergence frame excluded |
| Steady full presents | 52 | 0 | -100% |
| Steady copied bytes | 95,846,400 | 0 | -100% |
| Total retirement-confirmed bytes | 95,846,400 | 1,843,200 | -98.1% |
| Steady Slint raster | 6,566us | 5,701us | -13.2% |
| Steady custom-layer work | 896us | 750us | -16.3% |

The candidate's sole copy was an `identity-full` catch-up of an invalid
alternate scanout slot. It had no Slint damage and
`catchup_bytes == invalid_bytes == copied_bytes == 1,843,200`. Frames after
that convergence copied zero bytes. This required correctness copy is reported
separately and remains included in the total retirement-confirmed byte count.

Both arms passed with zero physical repeated vblanks, latch drops, ownership
losses, sequence gaps, and phase outliers.

## Controls

- Modal input passed:
  `build/agent-benchmarks/modal-input/1787371766/summary.json`
- The 40-second Arcade velocity control sustained at least 59.9 physical FPS
  with zero dropped frames, repeated vblanks, ownership losses, and latch
  drops:
  `build/agent-benchmarks/arcade-velocity-scroll/1787371788/summary.json`

## Disposition

Retain receipt-scoped modal carrier forcing as the sole production path. Keep
forcing for entry, pending retirement, stale receipts, and reconciliation; stop
only after the matching modal retirement receipt. Preserve the required
invalid-slot convergence copy and its explicit attribution.
