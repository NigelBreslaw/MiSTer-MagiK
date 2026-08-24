# Raw-scaler frame-integrity diagnostic design — 2026-08-24

## Decision

Replace the schema-2 raw RGB/activity observer with one passive schema-3
raw-control integrity recorder. Keep the queued scaler-completion repair and
all latch-v5 behavior unchanged.

## Evidence driving the change

The direct-Arcade campaign preserved one corrupt USB-video frame between two
byte-identical healthy MagiK frames. The authoritative RGB565 framebuffer was
correct, latch counters were clean, and the existing raw observer reported
saturated activity. The 30-second follow-up movie contained 755 healthy frames,
so the event self-cleared before a delayed activity snapshot could retain it.

The next useful question is narrow: did the ordered raw scaler CE/DE/HS/VS
waveform or its per-frame counts change during that transient? Pixel content,
PLL state, routing, framebuffer addresses, and scheduler state are deliberately
out of scope.

## Frozen design

- Observe only CE, DE, HS, and VS in `clk_hdmi`.
- Fingerprint ordered control samples with CRC-16 and count HS edges, DE starts,
  and active CE samples per completed frame.
- Establish a baseline after exactly three identical nonempty frames.
- Retain the first mismatch and its frame sequence until common reset/RBF reload.
- Export only read-only command `0x67`, schema 3; keep `0x60–0x66` unsupported,
  latch protocol 5, and capabilities `0x03ff`.
- Require three identical valid host records and always report sink visibility
  as unobserved.

The implementation may proceed to cached local Apple-container signoff only
after structural binding, patched-production GHDL, Icarus responder tests, and
the unchanged completion formal proof pass. It is experimental and must not be
sent to CI or installed before that local gate.

Source incident:
[Phase-2 transient corruption](2026-08-24-phase2-transient-corruption-result.md).
