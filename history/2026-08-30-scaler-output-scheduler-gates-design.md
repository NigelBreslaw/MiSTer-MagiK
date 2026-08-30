# Scaler output-scheduler gate diagnostic design — 2026-08-30

Status: implementation candidate. This design replaces schema 15 and must pass
all local structural, simulation, formal, coverage, and fixed-seed Apple
Quartus certification gates before any device action.

## Preserved incident and causal boundary

Platform-v0.37 reproduced moving horizontal corruption after a supervised
MagiK start. Two authoritative 960x540 RGB565 framebuffer captures were clean
and differed only in the clock. Native USB video showed a moving displaced
band. Three coherent schema-15 publications froze on `no_request_seen` after
normal fetch liveness. At that boundary the Avalon controller was idle, reset
was released, the retained-return drain was open and empty, and no external
read was presented. Latch ownership, route publication, posts, flips, and the
launcher remained healthy.

This rules the captured failure out of the framebuffer, latch transaction,
Avalon wait/return path, retained reset drain, and completion queue. It does
not identify which output-clock scheduler gate stopped new reads.

## Replacement observer

Read-only command `0x68` remains a four-word `{schema, flags, state, crc}`
record with magic `0x4d58`. Schema advances from 15 to 16 and the diagnostic
architecture becomes `scaler-output-scheduler-gates-v1`. The external
accepted-obligation scoreboard, reset-retentive watchdog, immutable
publication bank, CRC serializer, and two-stage acknowledgement handshake are
unchanged.

Only the existing 16-bit `magik_fetch_state` tap is replaced. When the external
watchdog proves `no_request_seen`, the immutable state word captures these
output-clock scheduler gates:

| Bits | Field | Meaning |
| --- | --- | --- |
| 1:0 | `output_state` | `sDISP`, `sHSYNC`, `sREAD`, or `sWAITREAD` |
| 3:2 | `copy_state` | `sWAIT`, `sSHIFT`, or `sCOPY` |
| 5:4 | `read_level` | outstanding output read metadata, range 0..2 |
| 7:6 | `copy_level` | completed blocks available to copy, range 0..2 |
| 8 | `address_ready` | delayed `o_adrsa` gate used by `sREAD` |
| 9 | `read_pending` | output request phase differs from synchronized acknowledgement |
| 10 | `read_toggle` | run-gated request phase presented to the Avalon crossing |
| 11 | `copy_write_active` | front copy-valid state `o_copyv(0)` |
| 12 | `copy_adturn` | copy address has crossed its terminal qualification point |
| 13 | `copy_shift_next` | next format/word phase is eligible |
| 14 | `copy_line_last` | delayed line terminal `o_last2` |
| 15 | `copy_terminal_ready` | exact production copy-retirement predicate |

The packed output is observational only. It has no assignment path into the
scaler, latch, route, reset, clock, pixel, memory, or completion logic. The
host rejects reserved output/copy states and read/copy levels above two. It
retains schema-14 and schema-15 decoding for rollback evidence.

## Decision table

The frozen snapshot separates the leading causes without guessing:

- `sREAD`, `read_level < 2`, and `address_ready = 0`: address pipeline gate
  failed to reopen.
- `sWAITREAD` with `read_pending = 1`: request acknowledgement path stopped.
- `sREAD` with `read_level = 2`: scheduler credits are saturated.
- saturated levels with `sWAIT` and `copy_level > 0`: copy-start gate is
  inconsistent.
- `sCOPY` with no exact terminal predicate: copy retirement is blocked before
  `lev_dec_v`.
- `copy_terminal_ready = 1` while levels remain saturated: the terminal branch
  or decrement/accounting transition failed.
- none of the above: evidence remains explicit and inconclusive; no functional
  patch is justified.

## Proof and certification sequence

Before Quartus, the candidate must pass the checked protocol generators,
warning-clean Icarus suites, Verilator lint, exact pinned-Menu patch
integration, GHDL production compilation and queue simulation, sys-top and
diagnostic responder simulation, exact-source Yosys bounded proof, temporal
induction, every required non-vacuity cover, and both Verilator coverage
closures. The fixed-seed Apple-container signoff then builds the matched stock,
pre-observer, and patched variants through `scripts/agent fpga signoff` only.

Certification success is the stop boundary. There is no merge, push, platform
publication, RBF installation, reboot, or device reproduction in this task.
