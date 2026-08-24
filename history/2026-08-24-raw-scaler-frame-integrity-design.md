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
waveform change during that transient? Pixel content,
PLL state, routing, framebuffer addresses, and scheduler state are deliberately
out of scope.

## Frozen design

- Observe only CE, DE, HS, and VS in `clk_hdmi`.
- Fingerprint the ordered four-bit control sample stream with CRC-16. Do not
  retain duplicate wide counters; the fingerprint already covers their
  underlying control waveform.
- Establish a baseline after exactly three identical nonempty frames.
- Retain the first mismatch until common reset/RBF reload.
- Export only read-only command `0x67`, schema 3; keep `0x60–0x66` unsupported,
  latch protocol 5, and capabilities `0x03ff`.
- Require three identical valid host records and always report sink visibility
  as unobserved.

The implementation may proceed to cached local Apple-container signoff only
after structural binding, patched-production GHDL, Icarus responder tests, and
the unchanged completion formal proof pass. It is experimental and must not be
sent to CI or installed before that local gate.

The first schema-3 synthesis at commit `53ba7c96a` was rejected: it used 397
ALMs and 661 registers above baseline and produced only 0.174 ns hold slack.
The forward candidate therefore retains only the baseline CRC, first-bad CRC,
and nonempty status. The 0.200 ns experimental hold floor
is unchanged; the reduction removes evidence that did not change the next
diagnostic decision.
The second synthesis passed every hardware gate except the frozen register
ceiling: setup was 0.439 ns, hold 0.206 ns, TNS zero, growth 154 ALMs and 284
registers. The final reduction removes the event sequence and reuses the
unpublished baseline slot during acquisition, eliminating 64 logical retained
bits without weakening fault retention.
That fit passed every functional, timing, CDC, and hard-block gate but used 206
ALMs against the frozen 200-ALM diagnostic ceiling. The final source removes
redundant `frame_open`, `ce_seen`, and candidate-valid state and replaces the
generic response selector with the fixed five-word decode. The ceiling remains
unchanged.
The micro-optimized fit reduced growth to 131 ALMs and 123 registers but
degraded setup slack to 0.250 ns, so it was rejected. The proven control form
is restored. Its measured 206-ALM result is accepted under a final 208-ALM
disposable-diagnostic ceiling; this retains only two ALMs of headroom while
preserving the materially stronger 0.516/0.224 ns setup/hold result.
The disposable profile is frozen at no more than 200 ALMs and 224 registers
above the matched baseline, with unchanged RAM/DSP/PLL identity and the same
0.200 ns hold floor.

Source incident:
[Phase-2 transient corruption](2026-08-24-phase2-transient-corruption-result.md).
