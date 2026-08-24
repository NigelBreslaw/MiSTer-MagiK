# Experimental scaler copy-retirement diagnostic

Schema 5 established a persistent physical MagiK black with a correct varied
framebuffer while the scaler reported `readlev=2`, `copylev=2`, no new reads or
returns, and zero data at every observed pixel stage. Copy-read, line-write,
and raw-active timing nevertheless continued. This disposable schema-6 probe
therefore replaces the broad pipeline observer and asks only why a full copy
queue is not retiring.

The queued completion repair and every production reset, latch, route, mux,
PLL, framebuffer, and pixel behavior remain unchanged. Diagnostic state is
output-only. No schema-5 observer or Avalon diagnostic crossing remains.

## Exact production boundary

The production copy FSM retires one buffered block only when this existing
predicate is true during an active `sCOPY` shift:

```text
o_adturn
and shift_onext((o_acpt + 1) mod 16, o_format)
and (((o_ad mod BLEN = 0) and not o_lastv(0)) or o_last2)
```

The same branch sets `o_copy <= sWAIT` and `lev_dec_v := '1'`. The observer
records that exact branch, the resulting level decrement, and the individual
terms needed to explain a predicate that never becomes true.

## Schema 6 record

Command `0x67` remains the only diagnostic command. Magic is `0x4d57`; the
four response words are schema `6`, flags, state, and the existing CRC-16.
Commands `0x60` through `0x66` remain unsupported. Latch protocol `5` and
capabilities `0x03ff` are unchanged. There is no arm, clear, freeze, write, or
recovery operation.

The flags word is sticky from one completed output frame boundary to the next:

| Bit | Event seen during the frame |
| --- | --- |
| 0 | completed-frame record valid |
| 1 | copy started from `sWAIT` with `o_copylev>0` |
| 2 | `lev_dec_v` asserted |
| 3 | copy FSM observed in `sWAIT` |
| 4 | copy FSM observed in `sSHIFT` |
| 5 | copy FSM observed in `sCOPY` |
| 6 | active copy shift (`hcarry_v or o_dshi>0`) |
| 7 | `o_adturn` seen during an active shift |
| 8 | next pixel phase required a word shift |
| 9 | `o_ad mod BLEN=0` and front `last=0` seen together |
| 10 | `o_last2` seen |
| 11 | exact terminal predicate branch taken |
| 12 | `o_ad` wrapped while in `sCOPY` |
| 13 | a later copy start repeated the first `{prim,last,bank,offset}` signature |
| 14 | a later copy start differed from the first signature |
| 15 | copied DPRAM word was nonzero on `o_sh3` |

Internal event bit 0 maps to exported flag bit 1; exported bit 0 is added only
when a complete frame is published. Events and frame publication occur in the
existing `Scalaire` process, so `lev_dec_v`, `hcarry_v`, and the terminal
predicate retain their exact same-edge production meaning. State fields are
the registered pre-edge values at the closing VS boundary:

| Bits | State |
| --- | --- |
| 1:0 | copy FSM: 0 wait, 1 shift, 2 copy |
| 3:2 | `o_readlev` |
| 5:4 | `o_copylev` |
| 6 | `o_adturn` |
| 7 | front `prim` metadata |
| 8 | front `last` metadata |
| 9 | front DPRAM bank |
| 13:10 | front pixel offset |
| 14 | `o_last2` line-terminal pipeline state |
| 15 | `o_copyv(0)` copy-output active |

The completed 32-bit record and generation toggle are stable in `clk_hdmi`.
The responder waits one HDMI edge, then transfers the bundle into `clk_sys`
with the existing generation synchronizer plus one-edge bundle-settling
pattern. UIO snapshots remain immutable for the transaction.

## Conservative host decisions

Three valid samples are required and must independently yield one stable
classification. Harmless state differences do not invalidate otherwise
matching evidence.

| Classification | Meaning |
| --- | --- |
| `scaler_copy_lev_dec_missing` | exact terminal branch was observed without its same-branch decrement |
| `scaler_copy_terminal_condition_stall` | a full frame showed active copy shifts, address turn and wrap, next-word phase, and terminal metadata, but neither terminal branch nor decrement |
| `scaler_copy_metadata_or_buffer_repetition` | blocks retired with zero copied data and every later start repeated the first metadata signature |
| `scaler_copy_buffer_selection_zero` | blocks retired with zero copied data, but metadata was not a pure repetition |
| `scaler_copy_retirement_active` | copy start, active shift, terminal branch, decrement, and nonzero copied data were all observed |
| `scaler_copy_retirement_evidence_inconclusive` | invalid, mixed, transitional, or insufficient evidence |

Healthy experimental activation accepts only
`scaler_copy_retirement_active`. Every result retains
`sink_visibility: "unobserved"`.

## Proof and signoff boundary

- GHDL compiles patched production `ascal.vhd` and calls the exact shared
  15-bit sticky accumulator for empty, one-hot, simultaneous, and retained
  events.
- Structural proof binds start, decrement, address-wrap, nonzero DPRAM, and the
  terminal event to the exact production FSM sites and rejects observer fanout
  into production assignments.
- Icarus proves schema-6 framing, CRC, immutable snapshots, reset/partial-read
  behavior, `0x60`–`0x66` unsupported, and latch-v5 non-interference.
- Completion queue GHDL/formal proof remains unchanged.
- Only one diagnostic CDC remains: 32 stable bundle bits and one generation
  toggle from HDMI to sys. Together with completion request and acknowledgement
  this is four bounded net-delay groups, 35 exact paths, and three calculable
  chains above the matched baseline. Every chain and their combined MTBF must
  remain at least `10^12` device-hours.
- No new RAM, DSP, or PLL is expected. Compared with schema 5, removing the
  Avalon bucket, its bundle and synchronizers, and broad stage flags should
  reduce both area and placement pressure. Quartus/Apple signoff is a separate
  root-owned gate and is not implied by the local proof.

The next implementation must be selected only from preserved physical-black
evidence. This probe is removed after the copy-retirement mechanism is known.
