# Raw-scaler ordered-signature v3 design — 2026-08-25

This final local candidate retains the schema-9 16-bit ordered recurrence but
removes two further synthesized pressure sources. The preserved isolation stage
stores the native RGB565 information—five red, six green, and five blue bits—
rather than eight expanded bits per channel. Production rendering is RGB565,
so the omitted low expanded bits add no independent source information. This
removes eight isolation registers and reduces pixel-token fanout.

The wrapping response sequence now advances in the system clock domain only
when the existing generation synchronizer and settle cycle coherently capture
a new published signature. It remains strictly advancing with wrap for the
three-record host check but no longer crosses as 16 source registers. Together
these changes remove 24 additional real observer registers while preserving
the full 24-bit production port as a read-only input, observer-only isolation,
read-only `0x67`, immutable capture, CRC-16/CCITT-FALSE, exact generation CDC,
latch-v5, capabilities `0x03ff`, and every protected production cone.

Schema 10 has five words: schema, flags, capture sequence, 16-bit ordered
signature, and CRC. Its architecture identity is
`raw-scaler-ordered-signature-v3`. The same conservative interpretation applies:
three stable records support downstream origin only with the exact
same-candidate healthy static scene and independently detected moving physical
corruption; changing records require byte-stable source proof before supporting
an at-or-before-`ascal` origin.
