# 2026-08-24 schema-5 scaler pipeline-state design

## Trigger

The schema-4 experimental RBF reproduced a genuine persistent MagiK black
screen after 74 clean returns. The authoritative framebuffer remained the
expected nonblack Arcade image, while three coherent completed active-frame
records reported raw scaler RGB as exactly black. Earlier schema-3 evidence had
already shown healthy CE/DE/HS/VS timing during physical black.

This leaves a narrow ambiguity inside the scaler: no read acceptance, no
current memory return, returned zero data, loss between DPRAM copy and
line-buffer write, or zeroing in vertical/raw-output processing. A downstream
pixel probe would skip that decision; another broad observer would add risk
without improving it.

## Frozen disposable design

Schema 5 removes the schema-4 RGB observer and exports one 32-bit completed
frame record from production `ascal.vhd`. Low bits record the ordered stage
activity/nonzero ladder; high bits record read/copy levels, completion
request/ack queue state, reset-return drain/accounting, scaler run/new-resolution,
and read-active state. The exact encoding and one-cycle alignment are in
[`docs/fpga-raw-scaler-diagnostic.md`](../docs/fpga-raw-scaler-diagnostic.md).

The Avalon activity bucket closes on the existing synchronized output-VS
boundary, crosses into HDMI as a stable bundle plus synchronized generation,
and is combined with the corresponding completed HDMI activity bucket. The
result crosses to `clk_sys` with the same bounded generation/stable-bundle
pattern. Only the responder reads it. There is no write, clear, arm, freeze,
reset, route, or recovery action and no observer signal feeds production logic.

## Quartus-17 boundary correction

The first implementation compiled in GHDL's VHDL-2008 mode but Quartus 17
analysis rejected five reads of the ascal OUT ports `o_de/o_r/o_g/o_b`. Port
modes were not weakened and production output assignments were not changed.
There is no single readable internal signal after ascal's mode, mask, and
border selection, so duplicating that selection would create a second logic
definition rather than an exact probe.

The forward correction removes every raw OUT-port read from VHDL. Ascal now
publishes only flags 0 through 9. One preserved HDMI-domain boundary stage
beside `sys_top` samples the unchanged raw RGB/DE/VS ports, retains only
completed-frame active/nonzero flags, and merges them with the stable ascal
record after a same-domain settling edge. The merged bundle alone crosses into
`clk_sys`. Structural proof explicitly rejects any future ascal OUT-port read;
Icarus proves raw active, raw zero, atomic merge, reset, and immutable response
behavior.

The Avalon bundle is exactly 13 physical registers for fields 0 through 12.
Quartus must find exactly those 13 source endpoints; no dummy preserved
registers exist for ABI-reserved bits.

Command `0x67` remains the only diagnostic command. Magic stays `0x4d57`, the
schema is `5`, the response is four words including the existing CRC, commands
`0x60–0x66` remain unsupported, and latch-v5/capabilities `0x03ff` remain
unchanged. Every host record states `sink_visibility: "unobserved"`.

## Decision ladder

Three valid completed-frame records must agree on the earliest missing/nonzero
stage. The host reports pipeline active, read scheduler stall, memory return
stall, returned zero data, copy-buffer stall/zero, line-buffer write zero,
vertical/output zero, or inconclusive evidence. Harmless level/toggle changes
do not require byte-identical records. Healthy activation accepts only pipeline
active; every other result fails closed.

At the next preserved physical black:

- no read acceptance points to scheduler/reset-epoch entry;
- accepted read without current returns points to memory traffic/return drain;
- current returns with no nonzero word points to returned data or addressing;
- later missing/nonzero boundaries identify the first copy, line-buffer, or
  vertical/raw-output stage needing the next narrow investigation;
- mixed or transitional evidence changes only this disposable sampling path.

No classification proves physical visibility or correct pixels.

## Local proof boundary

The production patch and package compile from the exact pinned Menu source.
Synthesis and GHDL share the exact event-accumulator functions. Directed GHDL
tests cover empty buckets, each individual stage bit, sticky accumulation, and
the unchanged completion queue. Icarus covers schema-5 framing, CRC, immutable
transaction snapshots, malformed/partial reads, unsupported commands, reset,
and latch-v5 non-interference. Structural checks require the exact CDC
endpoints, reject retained schema-4 taps, and reject observer fanout into
production cones. The unchanged completion BMC, required covers, and temporal
induction must remain green.

Quartus timing/resource/CDC signoff, device installation, and physical testing
are intentionally outside this implementation commit. No RAM, DSP, or PLL is
expected; the main integration risk is placement perturbation in dense
`ascal`, so the candidate is not device-ready until fixed-seed Apple-container
signoff passes.
