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

After candidate 4 passed timing/resources but Quartus duplicated its CDC source,
the forward implementation adds one dedicated HDMI-domain
`generation_launch` register. `source_generation` still toggles only with the
stable published signature. `generation_launch` follows it one HDMI clock
later and has exactly one data fanout, the existing `generation_meta` first
synchronizer stage. The bounded identity is therefore
`generation_launch -> generation_meta`; the two-stage synchronizer, settle
cycle, immutable response, schema, and signature datapath are unchanged.

Fixed-seed candidates 5 and 6 proved that the additional launch register itself
caused an identical timing/resource failure regardless of the
`dont_replicate` hint. The forward candidate therefore removes that rejected
stage and returns to candidate 4's timing-clean source topology. The existing
`source_generation` register is marked non-replicable, while the exact report
gate rejects `source_generation~DUPLICATE`. The signature datapath, schema,
two-stage synchronizer, destination settle cycle, and immutable response do not
change.

## Qualified implementation and device result

The final candidate adds one further observer-only HDMI pipeline stage after
the direct isolation boundary. It captures only on isolated scaler CE, so it
preserves the ordered RGB565 samples while removing unnecessary observer
switching. This placement passed the unchanged local simulation, exact-source
completion and copy-tail proofs, structural checks, and fixed-seed Apple
signoff: setup `+0.724 ns`, hold `+0.246 ns`, zero TNS, 158/158 constrained
relationships, `+188` ALMs, `+191` registers, exact 3/3 diagnostic CDC paths,
and unchanged RAM/DSP/PLL identity. The exact artifacts are recorded in the
[device incident](2026-08-25-raw-scaler-signature-v3-corruption-incident-v1.json).

The installed candidate passed its initial physical smoke and 44 Phase 2
returns across eight bounded reboot boundaries. Return 45 initially failed the
host temporal detector. Native review then proved that all three stills showed
the complete normal Arcade launcher and the following 749-frame movie was
healthy. The second confirmation used full-range luma `0..255` while the first
two used video-range `16..235`; the unnormalized grid comparison promoted that
range change to a false corruption result.

The coherent direct-`ascal` records and byte-identical framebuffer remain valid
identity evidence, but there was no physical failure to classify. The earlier
downstream-of-`ascal` conclusion is retracted. No post-OSD probe is justified by
this event. Phase 2 must rerun with the range-normalized static-region detector
before another FPGA diagnostic is designed.
