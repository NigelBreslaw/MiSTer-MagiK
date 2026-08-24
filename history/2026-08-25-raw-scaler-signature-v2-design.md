# Raw-scaler ordered-signature v2 design — 2026-08-25

## Scope

This genuinely different disposable candidate follows two rejected schema-8
fits. It changes no production RTL, seed, fitter setting, timing/resource gate,
CDC constraint, protected cone, latch-v5 behavior, or capability bit. The sole
production consumers remain a preserved isolation stage for direct `ascal` CE,
24-bit RGB, DE, HS, and VS.

## Synthesized-state reduction

Schema 9 retains read-only command `0x67`, magic `0x4d57`, frame-valid flags,
the wrapping 16-bit source sequence, CRC-16/CCITT-FALSE, and an immutable
response. Its five words are schema, flags, sequence, ordered signature, and
CRC. The architecture identity is `raw-scaler-ordered-signature-v2`.

The ordered signature is 16 bits. Every qualified pixel or line-end still
consumes exactly one ordered update. The 32-bit RGB/boundary token is folded by
XORing its halves before one reflected Galois step using polynomial `0xa001`.
The independent executable model uses the same token stream but a separately
implemented recurrence.

Narrowing frame, published, and system snapshot signature state from 32 to 16
bits removes 48 synthesized registers. The stable CDC bundle is now exactly 32
bits: signature plus source sequence. The two-stage generation synchronizer,
one settle cycle, destination valid bit, command-time capture exclusion, and
immutable CRC response are unchanged.

## Interpretation boundary

The host still requires three valid, strictly advancing frames. Equal signatures
support `raw_scaler_ordered_stable`; unequal signatures require independent
byte-stable source proof before supporting an at-or-before-`ascal` origin.
Because the disposable signature is 16 bits, stable evidence supports a
downstream origin only when all three records match the exact same-candidate
healthy static scene and the committed temporal detector independently proves
moving physical corruption during the capture interval. Anything less remains
inconclusive. The FPGA never proves sink visibility.
