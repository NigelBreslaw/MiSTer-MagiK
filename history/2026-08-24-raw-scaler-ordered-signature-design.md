# Raw-scaler ordered-signature diagnostic design — 2026-08-24

## Rejected predecessor

The schema-7 ordered CRC candidate failed fixed-seed local signoff before any
device action. All 25 reported setup violations were internal CRC feedback;
the worst path was `frame_crc[4]~DUPLICATE` to `frame_crc[31]`, with a
`6.732 ns` relationship, `9.260 ns` data delay, and `-2.854 ns` slack. The
observer used 647 combinational ALUTs and 468 registers; matched total growth
was 586 ALMs and 624 registers. It is replaced rather than layered.

## Exact question and boundary

Schema 8 asks the same single question: while the independently captured fixed
source framebuffer and scene remain byte-stable, does the ordered raster at the
direct `ascal` boundary change during physical moving corruption?

The sole production taps remain `scaler_out` CE and direct `ascal` RGB, DE, HS,
and VS. One preserved HDMI-clock register stage isolates those taps. Observer
signals have no fanout into scaler, completion, copy-tail, framebuffer, latch,
route, reset, OSD, mux, PLL, or output logic. Command `0x67` is read-only;
`0x60` through `0x66` remain unsupported; latch-v5 and capabilities `0x03ff`
remain unchanged.

## Shallow ordered signature

Each qualified active-pixel or line-end sample produces one 32-bit token from
RGB, line-start, line-end, and HS. Exactly one reflected Galois step using
polynomial `0x82f63b78` consumes that token per HDMI edge. The signature starts
at `0x6d5a56da` for every frame. Empty frames do not publish.

The response is schema 8, magic `0x4d57`, and six words: schema, valid flags,
wrapping frame sequence, ordered-signature low/high, and CRC-16/CCITT-FALSE.
Only the latest completed nonempty frame is retained. A two-stage generation
synchronizer and one settle cycle transfer an immutable 64-bit bundle to the
system-clock responder. The response snapshot has no blanket preserve.

The host requires three valid records with strictly advancing sequences.
Identical signatures support only `raw_scaler_ordered_stable`. Changing
signatures require independent exact static-source proof before supporting an
at-or-before-`ascal` origin. Stable signatures matching the exact same-candidate
healthy scene while native USB video moves support a downstream origin. Missing
or ambiguous source, scene, sequence, transport, or physical evidence remains
inconclusive; sink visibility is always unobserved by the FPGA.

## One-retry gates

Simulation must bind an independent executable signature model and cover pixel,
line-order, empty-frame, reset, wrap, immutable-read, partial-read, CRC, and
unsupported-command behavior. Existing copy-tail and completion formal proofs
remain mandatory. Structural checks require exact taps, isolation, observer-only
fanout, and unchanged protected cones.

The checked-in local profile is `experimental_raw_scaler-v1`. It changes no
production default and permits only the existing disposable limits: setup at
least `0.350 ns`, hold at least `0.200 ns`, setup degradation at most `0.300 ns`,
zero TNS, at most 208 ALMs and 224 registers, unchanged RAM/DSP/PLL/warnings,
and exact three-chain endpoint/net-delay/MTBF evidence. Only the aggregate
placement-sensitive synchronizer total may vary. One seed-2 Apple signoff is
allowed; no sweep, waiver, fitter change, or device action may rescue failure.
