# Raw-scaler ordered-frame diagnostic design — 2026-08-24

## Question and boundary

The preserved USB-video movie shows a slowly descending band of repeated or
missing-looking rows while the authoritative RGB565 framebuffer remains
correct. Existing scheduler, copy-tail, completion-queue, latch-v5, and route
evidence remains coherent. This experimental RBF answers one question only:
does the ordered raster already change at the direct `ascal` output while the
known static source framebuffer remains byte-stable?

The observer is passive. Its only production taps are the `clk_hdmi`-domain
`scaler_out` clock enable and direct `ascal` `hdmi_de`, `hdmi_hs`, `hdmi_vs`,
and `hdmi_data[23:0]` outputs. One explicit observer-only register stage
isolates those nets from the CRC cone. No observer signal may feed the scaler,
copy-tail, completion queue, framebuffer, latch, reset, route, shadowmask,
OSD, mux, PLL, or production pixel/output logic. There is deliberately no
second fingerprint at the final HDMI boundary.

## Ordered CRC-32C encoding

CRC-32C uses reflected polynomial `0x82f63b78`, initial value `0xffffffff`, and
final XOR `0xffffffff`. Only isolated samples for which the delayed
`scaler_out` clock enable is asserted are consumed.

Each completed nonempty frame is encoded as this byte stream:

1. frame-start delimiter `f0`;
2. for every active line, line-start delimiter `a0 | hdmi_hs`;
3. for every active pixel, pixel tag `01` followed by R, G, and B bytes;
4. line-end delimiter `a2 | hdmi_hs` after the last active pixel of each line;
5. frame-end delimiter `f1`.

Frame boundaries are rising `hdmi_vs` samples and active-line boundaries are
`hdmi_de` transitions. The isolated `hdmi_hs` level at each DE boundary is
encoded in the low delimiter bit. Active RGB ordering, explicit DE-derived
line delimiters, and VS-derived frame delimiters therefore encode the raster
under test without making blanking porch lengths part of the fingerprint.

The frame ending on a boundary contains samples before that boundary. Empty
frames do not advance published sequence, CRC history, geometry, or variation
history. Pixel count is the number of active samples; line count is the number
of DE active runs containing at least one active sample.

## Read-only ABI

Command `0x67` is `raw-scaler-ordered-frame-v1`; commands `0x60` through `0x66`
remain unsupported. The response magic is `0x4d57`, schema is `7`, and the
command-start response snapshots one immutable record until `io_uio` is
released. Latch protocol `5` and capabilities `0x03ff` remain unchanged.

After magic, the fixed response words are:

| Word | Meaning |
|---:|---|
| 0 | schema (`7`) |
| 1 | flags: bit 0 coherent completed-frame evidence; bit 1 nonempty; bit 2 variation-window full; bit 3 variation count saturated/reserved; all other bits zero |
| 2 | completed nonempty frame sequence, wrapping 16-bit |
| 3 | active pixel count bits 15:0 |
| 4 | active pixel count bits 23:16 in bits 7:0; bits 15:8 zero |
| 5 | active line count bits 11:0; recent CRC-variation count (`0..8`) in bits 15:12 |
| 6 | newest completed-frame CRC-32C bits 15:0 |
| 7 | newest CRC bits 31:16 |
| 8 | previous completed-frame CRC bits 15:0 |
| 9 | previous CRC bits 31:16 |
| 10 | oldest retained completed-frame CRC bits 15:0 |
| 11 | oldest retained CRC bits 31:16 |
| 12 | CRC-16/CCITT-FALSE over the existing evidence header and words 0..11 |

The variation count is the population count of an eight-comparison sliding
window. Each bit records whether a completed nonempty frame CRC or geometry
differs from its predecessor. The window-full flag becomes true after eight
comparisons. Retention order is newest, previous, oldest.

The HDMI-domain completed-frame state changes atomically and toggles a
generation bit. The `clk_sys` receiver uses a two-stage synchronizer, waits one
additional system clock for the bundled state to settle, and then copies it.
The UIO responder snapshots only that coherent system-domain copy.

## Host decision contract

The host takes three valid records at bounded intervals. It rejects malformed,
partial, noncanonical, CRC-invalid, incoherent, unsupported, or geometrically
impossible records. It reports only internal raster evidence and continues to
state `sink_visibility: "unobserved"`.

- Changing ordered CRC or geometry while an independently captured known
  static framebuffer and scene are stable supports `raw_scaler_order_changed`,
  locating the fault at or before direct `ascal` output.
- Stable ordered evidence matching an exact same-scene healthy reference while
  the physical USB image moves supports `downstream_of_raw_scaler`.
- Stable ordered evidence differing from that exact healthy reference supports
  `persistent_raw_scaler_phase_or_epoch`.
- Missing static-source proof, a scene transition, changing record transport,
  insufficient history, or any ambiguity is
  `raw_scaler_ordered_evidence_inconclusive`.

Neither this ABI nor its host decoder infers physical HDMI visibility.

## Proof and candidate limits

Patched-production GHDL must cover the exact encoding, delimiters, empty
frames, geometry, three-frame retention, sequence wrap, variation-window
behavior, and reset in every observation phase. Icarus must cover immutable
snapshots, framing, CRC, partial reads, `0x60`–`0x66` non-response, and latch-v5
non-interference. Structural checks must prove the exact taps, isolation stage,
observer-only fanout, and unchanged protected cones. Existing completion and
copy-tail exact-source proofs remain unchanged and mandatory.

Expected disposable diagnostic cost before synthesis is approximately 500
registers and 150–250 ALMs, dominated by the three retained CRCs, coherent CDC
snapshot, and 32-bit CRC transform. This is intentionally larger than the
retired activity probes but materially narrower than a second final-output
fingerprint. Quartus signoff, device installation, and physical interpretation
are separate later gates and are not authorized by this implementation step.
