# Experimental scaler pipeline-state diagnostic

This disposable RBF observer answers the next narrow question raised by the
schema-4 black-screen result: at which scaler pipeline boundary does activity
or nonzero data first disappear? It is evidence only, not a production feature
or recovery mechanism.

The queued completion repair and latch-v5 remain unchanged. The schema-4 raw
RGB observer is removed rather than retained or stacked. Stages through the
line buffer are observed inside production `ascal.vhd` using the exact
registered signal at the consuming stage. Raw DE/RGB alone are sampled at the
unchanged ascal boundary beside `sys_top`. Every diagnostic output flows only
to the read-only responder beside the latch bridge.

## Per-frame activity record

Bits `11:0` are sticky within one completed frame:

| Bit | Exact event observed |
| --- | --- |
| 0 | completed-frame record valid |
| 1 | read obligation accepted: production `read_obligation_accept(...)` |
| 2 | `avl_readdatavalid` accepted while reset-return drain is inactive |
| 3 | that accepted 128-bit Avalon return was nonzero |
| 4 | returned beat created a BLEN completion |
| 5 | destination completion pulse `o_readdataack` |
| 6 | copy/DPRAM read stage `o_sh3` |
| 7 | copied DPRAM word `o_dr` was nonzero on an `o_sh3` edge |
| 8 | line-buffer write enable `o_wr` was nonzero |
| 9 | line-buffer write pixel `o_ldw` was nonzero on an `o_wr` edge |
| 10 | raw boundary `hdmi_de` was active |
| 11 | raw boundary `hdmi_data[23:0]` was nonzero while DE was active |

Copy and line-write observations use the same registered values consumed by
`Scalaire` and `OLBUF` on that edge. Quartus 17 rejects reads of the VHDL OUT
ports `o_de/o_r/o_g/o_b`, and ascal has no single internal signal representing
the final mode/border-selected RGB. The raw flags therefore use one preserved
schema-4-style HDMI-domain input stage outside ascal. Rising staged VS closes
the preceding raw frame and is excluded from it. No first pixel, variation
state, content baseline, or schema-4 record remains. The internal accumulator
functions live in the package compiled from patched production `ascal.vhd`;
the GHDL test and synthesis call those same functions.

The Avalon bucket closes on the existing synchronized output-VS boundary. Its
stable 13-bit bundle contains exactly fields 0 through 12 and is published with
a generation toggle; no constant reserved registers are synthesized. The HDMI
receiver
uses an explicit two-stage generation synchronizer and waits one further edge
before capture. Ascal publishes an exact 25-bit physical record in `clk_hdmi`:
state bits 14 through 0 followed by internal flags 9 through 0. Reserved state
bit 15 and flags 15 through 12 are not implemented as dummy registers. The
responder reconstructs their canonical zero values, waits one same-domain edge
after the ascal generation,
merges the already completed external raw-frame flags 10 and 11, and only then
publishes one stable 32-bit bundle into `clk_sys` through the toggle, two-stage
synchronizer, and one-edge settling pattern. A UIO transaction snapshots the
bundle immutably.

The high state word is:

| Bits | Meaning |
| --- | --- |
| 1:0 | `o_readlev` |
| 3:2 | `o_copylev` |
| 4 | completion request toggle |
| 5 | completion pending |
| 6 | synchronized source acknowledgement |
| 7 | HDMI destination-observed request |
| 8 | reset-return drain active |
| 10:9 | retained Avalon return credits |
| 11 | retained return phase nonzero |
| 12 | scaler running |
| 13 | new-resolution transition active |
| 14 | Avalon read active |
| 15 | reserved zero |

Request and pending are the just-computed queue state at the Avalon frame
boundary. Acknowledgement, drain, credits, phase, and read-active are pre-edge
state. HDMI read/copy levels and run/new-resolution are sampled when the stable
Avalon bundle is captured. These fields refine a stage classification; they do
not independently prove a cause.

## Read-only ABI

Command `0x67` exposes `scaler-pipeline-state-v1`, schema `5`. Commands `0x60`
through `0x66` remain unsupported. Latch protocol `5` and capabilities `0x03ff`
are unchanged. After magic `0x4d57`, the fixed four-word response is schema,
flags, state, and the existing framed CRC-16. There is no write, clear, arm,
freeze, reset, or recovery operation.

The device agent reads three CRC-valid completed-frame records at bounded
intervals. Records need not be bit-identical when harmless queue phase or
toggle fields change, but all three must select the same earliest missing or
zero stage while owner and launcher context remains stable.

| Classification | Conservative meaning |
| --- | --- |
| `scaler_pipeline_active` | every activity and nonzero stage was seen |
| `scaler_read_scheduler_stall` | no accepted read obligation |
| `scaler_memory_return_stall` | a read was accepted but no current return arrived |
| `scaler_returned_zero_data` | returns arrived but no returned word was nonzero |
| `scaler_copy_buffer_stall_or_zero` | completion/copy activity or copied nonzero data disappeared |
| `scaler_linebuffer_write_zero` | copy data was nonzero but line-buffer write/nonzero data disappeared |
| `scaler_vertical_or_output_zero` | line-buffer data was nonzero but active/nonzero raw output disappeared |
| `scaler_pipeline_evidence_inconclusive` | invalid, malformed, transitional, or mixed evidence |

Healthy experimental activation accepts only coherent
`scaler_pipeline_active`. Every result retains
`sink_visibility: "unobserved"`; internal activity never proves correct pixels
or a visible sink.

## Local gates and risk

- The production patch must compile under GHDL, and the shared exact activity
  accumulators must prove empty, individual-stage, sticky, and reset behavior.
- The existing completion queue GHDL, BMC, non-vacuity covers, and induction
  remain unchanged and passing.
- Icarus must prove schema-5 framing, immutable transaction snapshots, CRC,
  malformed/partial reads, `0x60–0x66`, and latch-v5 non-interference.
- Structural proof rejects retained schema-4 taps and any observer fanout into
  completion, framebuffer, latch, route, reset, PLL, mux, or final-output cones.
- Both bundled-data crossings and their generation synchronizers have explicit
  endpoints and bounded net delay under the existing MTBF policy.
- Quartus must resolve exactly 13 Avalon bundle registers and exactly 25 ascal
  capture registers; constant ABI-reserved bits are reconstructed externally.
- No RAM, DSP, or PLL is expected. The record uses small sticky flag buckets,
  two 16-bit stable bundles, one 32-bit bundle, synchronizers, and responder
  snapshot state; Quartus resource and timing signoff remains mandatory before
  device installation.

## Why schema 5 is next

Schema 3 reproduced physical black with stable CE/DE/HS/VS timing. Schema 4
then reproduced a persistent physical MagiK black screen with correct
framebuffer content and three coherent completed active frames whose raw scaler
RGB was exactly black. That places the black frame at or before the raw ascal
output but does not distinguish a scheduler stall, absent returns, zero memory
data, copy/line-buffer loss, or vertical/output zeroing. Schema 5 follows those
boundaries in order without exporting any pixel bus or wide content digest.

The frozen implementation rationale is recorded in
[`history/2026-08-24-scaler-pipeline-state-design.md`](../history/2026-08-24-scaler-pipeline-state-design.md).
The decisive schema-4 result remains in
[`history/2026-08-24-raw-rgb-black-result.md`](../history/2026-08-24-raw-rgb-black-result.md).
