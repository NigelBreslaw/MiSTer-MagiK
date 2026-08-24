# Raw-scaler RGB-state diagnostic design — 2026-08-24

## Decision

Replace the schema-3 raw-control fingerprint with one passive schema-4
completed-frame RGB-state observer. Preserve the queued completion repair,
latch-v5, route, reset behavior, and all production pixel logic unchanged.

## Evidence driving the boundary

The schema-3 RBF reproduced a persistent physical MagiK black screen while two
independent records retained the healthy CE/DE/HS/VS baseline. The authoritative
RGB565 framebuffer was correct, ownership remained stable, and latch drops and
rejects remained zero. The device incident remains preserved and unrecovered.

Production inspection resolves the raw boundary exactly: `hdmi_data[23:0]` is
the direct ascal `{R,G,B}` 8:8:8 output, and black is `24'h000000`. The next
minimal question is whether those active raw pixels are black/constant or
varied. A content baseline is intentionally rejected because the diagnostic
RBF reloads on return and healthy UI content changes.

## Frozen implementation

- Observe only raw RGB, raw DE, and raw VS in `clk_hdmi`.
- Publish the most recently completed frame's active-seen, any-nonblack,
  variation-seen, and exact first active RGB fields.
- Publish empty/no-DE frames as invalid; explicitly reset every observer and
  responder phase.
- Reuse only command `0x67`, magic `0x4d57`, and existing CRC framing with
  schema 4. Keep `0x60–0x66` unsupported and latch protocol/capabilities at
  `5`/`0x03ff`.
- Accept healthy experimental activation only for coherent `raw_rgb_varied`.
  Reject black, constant, or inconclusive activation evidence.
- Always retain `sink_visibility: "unobserved"`.

The candidate may proceed to Apple-container synthesis only after the
production structural check, GHDL/Icarus simulations, generated contracts,
Rust gates, and unchanged completion formal proof pass. No device, CI, or
Quartus action is part of this implementation step.

Source incident:
[schema-3 persistent black result](2026-08-24-frame-integrity-black-result.md).

## Timing-isolation revision

The first schema-4 Apple fit passed hold, TNS, CDC/MTBF, hard-block identity,
and area, but setup reached only `0.289 ns` against the `0.428 ns` gate. The
worst paths were unrelated internal ascal paths, showing placement
perturbation from the direct 24-bit comparison fanout rather than an observer
path failure.

The forward revision adds one preserved, coherent RGB/DE/VS register boundary
in `clk_hdmi`. Production raw outputs now drive only that shallow stage, while
all comparisons and frame delimiting operate on its consistently delayed
values. Schema, response, classifications, CDC transport, and production
logic remain unchanged. The 26 added boundary bits keep the expected total at
about `+178` registers, below the disposable `+224` cap.
