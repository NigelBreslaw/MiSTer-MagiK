# Experimental raw-scaler RGB-state diagnostic

This disposable RBF observer answers one question during the next persistent
physical MagiK black screen: is the raw scaler RGB itself black, constant, or
varied? It is evidence only, not a production feature or recovery mechanism.

The queued completion repair and latch-v5 remain unchanged. The schema-3
CE/DE/HS/VS control-CRC observer is removed rather than retained or stacked.

## Passive boundary

Production `sys_top` maps the direct ascal output to `hdmi_data[23:0]` as
`R[23:16]`, `G[15:8]`, and `B[7:0]`. Its exact black value is `24'h000000`.
The observer reads only that raw RGB bus, raw DE, and raw VS for completed-frame
delimiting. It does not read scheduler/completion state, framebuffer data or
addresses, latch or route state, reset control, PLL, mux control, post-OSD/final
pixels, or TMDS. Its outputs reach only the read-only UIO responder.

One explicitly preserved `clk_hdmi` input stage captures RGB, DE, and VS as a
coherent bundle. The production ascal outputs therefore drive only shallow
diagnostic flip-flop inputs; black/variation comparisons and frame delimiting
use the consistently one-cycle-delayed bundle. This timing isolation does not
change the sampled frame or the ABI.

For the current frame the HDMI-domain logic retains only:

- whether an active DE sample was seen;
- whether any active pixel differed from exact black;
- whether any active pixel differed from the first active RGB sample;
- the exact first active 24-bit RGB sample.

At rising raw VS, the previous completed frame is atomically published. An
empty/no-DE frame is published as invalid so stale evidence cannot be reused.
The next frame starts immediately with cleared state. There is no content
baseline or sticky comparison: the RBF reloads on return, black may occur from
its first complete frame, and healthy UI pixels legitimately change.

## Read-only ABI

Command `0x67` exposes `raw-scaler-rgb-state-v1`, schema `4`. Commands `0x60`
through `0x66` remain unsupported. Latch protocol `5` and capabilities `0x03ff`
are unchanged.

After magic `0x4d57`, the fixed five-word response is:

| Word | Meaning |
| --- | --- |
| 0 | schema `4` |
| 1 | flags: completed-frame valid, active seen, any nonblack, variation seen |
| 2 | first active RGB bits `15:0` (`G`, `B`) |
| 3 | first active RGB bits `23:16` (`R`); high byte reserved zero |
| 4 | existing framed CRC-16 |

The complete 48-bit HDMI bundle is registered before its generation toggle
changes. The `clk_sys` receiver synchronizes only that toggle through two
explicit stages, waits one additional edge, and copies the stable bundle. Each
UIO transaction uses an immutable snapshot. The generation is transport state,
not part of the record and does not need to remain unchanged across host reads.

## Host classification

The device agent reads three records at bounded 25 ms intervals. All three must
be CRC-valid completed active frames with stable classification fields.

| Classification | Exact requirement | Decision after a confirmed physical black |
| --- | --- | --- |
| `raw_rgb_black` | no nonblack and no variation; first RGB exactly `0x000000` in all samples | investigate scaler fetch, pixel-data, or reset-epoch behavior |
| `raw_rgb_constant` | no variation; one stable nonblack first RGB in all samples | investigate scaler fetch, pixel-data, or reset-epoch behavior |
| `raw_rgb_varied` | variation and nonblack observed in all samples; first RGB may change | move downstream to a minimal final-output probe |
| `raw_rgb_evidence_inconclusive` | empty, invalid, malformed, mixed, or unstable evidence | repair only this sampling or transport |

Healthy experimental activation is fail-closed: only coherent
`raw_rgb_varied` evidence is accepted. Black, constant, and inconclusive
evidence reject activation. Every result retains
`sink_visibility: "unobserved"`; varied internal RGB does not prove correct
pixels or a visible sink.

## Local gates before synthesis

- Apply the production patch and compile patched production `ascal.vhd`.
- Keep the exact completion-queue GHDL simulation, BMC, required covers, and
  induction unchanged and passing.
- Simulate varied, all-black-from-first-frame, constant nonblack, changing-first
  sample, empty/no-DE, reset during accumulation and response, atomic
  completed-frame publication, immutable transaction snapshots, CRC, malformed
  reads, `0x60–0x66`, and latch-v5 non-interference.
- Structurally reject the schema-3 control CRC/baseline, any production fanout,
  and any new completion, framebuffer, latch, route, reset-control, PLL, mux,
  final-pixel, or TMDS tap.
- Require exact generation synchronizer endpoints and bounded generation and
  48-bit bundled-data paths under the existing MTBF policy.
- Infer no RAM, DSP, or PLL and remain comfortably below the disposable
  schema-3 cap of `+208` ALMs and `+224` registers.
- Do not run CI or install this RBF until the committed candidate later passes
  cached Apple-container signoff. No seed sweep, waiver, fitter change,
  LogicLock, or direct Quartus command is permitted.

## Why schema 4 is next

The schema-3 RBF reproduced a persistent physical black screen while its raw
CE/DE/HS/VS fingerprint remained at the healthy baseline in two independent
three-sample records. The framebuffer remained correct and latch ownership and
counters remained healthy. That result excludes a raw control-waveform change
within the probe's coverage, but schema 3 deliberately did not observe RGB.

The preserved unrecovered incident and exact candidate identity are in
[`history/2026-08-24-frame-integrity-black-result.md`](../history/2026-08-24-frame-integrity-black-result.md).
