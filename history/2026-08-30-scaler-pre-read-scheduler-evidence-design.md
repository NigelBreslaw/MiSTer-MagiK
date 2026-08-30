# Scaler pre-read scheduler evidence design — 2026-08-30

Status: implementation candidate. This design replaces schema 16 and must pass
all local structural, generated-contract, simulation, formal, coverage, and
fixed-seed Apple Quartus certification gates before any device action.

## Preserved incident and causal boundary

The live moving-corruption recurrence was preserved without a reboot, core
load, launcher restart, delivery, or device write. The authoritative 960x540
RGB565 hidden-slot framebuffer was clean while native USB video showed moving
horizontal bands and a corrupt green lower region. Latch posts and flips kept
advancing with zero drops and rejects.

Schema 16 returned three CRC-valid, advancing, ownership-stable publications
with `normal_liveness_seen`, `no_request_seen`, no reset after normal
liveness, and no observer fault. Its frozen output-scheduler word was zero:
the instantaneous sample landed in `sDISP`, with `sWAIT`, empty read/copy
levels, and no request phase. This rules out the framebuffer, latch, Avalon
wait/return path, retained-return drain, completion queue, and copy tail. It
does not distinguish a missing horizontal-start event from an unterminated
vertical iteration, a deliberately skipped vertical read, or a failure after
the scheduler enters its read path.

## Replacement observer

Read-only command `0x68` remains a four-word `{schema, flags, state, crc}`
record with magic `0x4d58`. Schema advances from 16 to 17 and the diagnostic
architecture becomes `scaler-pre-read-scheduler-evidence-v1`. The external
accepted-obligation scoreboard, reset-retentive watchdog, immutable
publication bank, CRC serializer, and two-stage acknowledgement handshake are
unchanged. Schema 14, 15, and 16 remain rollback-decodable by the host.

Only the existing 16-bit `magik_fetch_state` interface is replaced. The vector
becomes an output-clock sticky summary with no functional fanout. The last
acceptance acknowledgement for a scaler line starts each new summary window;
mid-line burst acknowledgements leave it intact. The summary then records
monotonic evidence until the next completed line. Consequently, when the
external watchdog proves that no new request has appeared, the packed word
describes the complete interval after the last successful line rather than one
arbitrary output-clock cycle.

| Bit | Field | Sticky event since the last completed scaler line |
| --- | --- | --- |
| 0 | `window_valid` | a final burst acknowledgement started this evidence window |
| 1 | `output_enable_seen` | output pixel clock-enable `o_ce` was observed |
| 2 | `horizontal_sync_edge_seen` | the scheduler's `o_hsv(0:1)` rising edge occurred |
| 3 | `horizontal_start_seen` | `sDISP` consumed the latched `o_hsp` start event |
| 4 | `hsync_state_seen` | the scheduler executed `sHSYNC` |
| 5 | `vertical_iteration_seen` | the vertical accumulator requested another iteration |
| 6 | `vertical_decision_seen` | the vertical loop reached its read/skip decision |
| 7 | `read_entry_seen` | the exact `sHSYNC` read-entry predicate was true |
| 8 | `no_read_exit_seen` | `sHSYNC` returned to `sDISP` without a read |
| 9 | `skip_vertical_pixel_seen` | a no-read exit had `o_vpe = 0` |
| 10 | `skip_vertical_carry_seen` | a no-read exit had `o_vcarrym = false` |
| 11 | `read_state_seen` | the scheduler executed `sREAD` |
| 12 | `address_ready_seen` | delayed address readiness `o_adrsb` was observed in `sREAD` |
| 13 | `request_issue_seen` | the exact request-toggle branch executed |
| 14 | `wait_read_state_seen` | the scheduler executed `sWAITREAD` |
| 15 | `vertical_size_zero_seen` | the derived vertical output size was zero |

The internal source is cleared only by output reset or the final acknowledgement
of a scaler line. It stores ten independent facts; a registered projection
reconstructs six causally implied fields in the same 16-bit external sticky
word. This preserves the schema while avoiding milestone comparators and
combinational clock crossings. Event accumulation begins on the following output clock so prior-line
state cannot leak into the new interval. The no-request interval therefore
leaves a stable multi-bit CDC source long before the 100 MHz watchdog freezes
it. Impossible event orderings fail closed in the host decoder.

## Decision table

For a coherent `no_request_seen` freeze after normal liveness:

- no `window_valid`: request-delimited evidence is invalid;
- no `output_enable_seen`: output clock enable stopped after the last request;
- output enable without `horizontal_sync_edge_seen`: the horizontal sweep did
  not produce its scheduler edge;
- a scheduler horizontal edge without `horizontal_start_seen`: the `o_hsp` latch/consume
  boundary failed;
- `sHSYNC` plus vertical iterations without a vertical decision: the vertical
  accumulator loop did not terminate; `vertical_size_zero_seen` identifies the
  explicit zero-size case;
- a no-read exit with `skip_vertical_pixel_seen`: the active vertical-region
  gate stayed closed;
- a no-read exit with `skip_vertical_carry_seen`: the accumulated vertical
  carry gate stayed closed;
- read entry without `address_ready_seen`: the delayed address pipeline did
  not become ready;
- address readiness and request issue without an external Avalon request: the
  request phase failed between the scaler and Avalon boundary;
- any contradictory or incomplete ordering remains explicitly inconclusive.

## Frozen interfaces and resource boundary

There is no new opcode, command word, acknowledgement, clock domain, reset,
port width, latch capability, platform protocol, pixel tap, return-data tap,
or functional recovery path. The diagnostic adds ten compressed sticky facts
and a registered schema-17 projection at the already-exported scaler observer
port. The fixed seed, timing,
TNS, relationship, ALM,
register, RAM/DSP/PLL, and MTBF gates remain unchanged.

## Proof and certification sequence

Before Quartus, the candidate must pass the checked protocol generators,
warning-clean Icarus suites, Verilator lint, exact pinned-Menu patch
integration, GHDL production compilation and queue simulation, sys-top and
diagnostic responder simulation, exact-source Yosys bounded proof, temporal
induction, every required non-vacuity cover, and both Verilator coverage
closures. The fixed-seed Apple-container signoff then builds or reuses only
identity-matching stock and pre-observer variants and synthesizes the exact
committed candidate through `scripts/agent fpga signoff`.

Certification success is the stop boundary. There is no merge, push, platform
publication, RBF installation, reboot, or device reproduction in this task.
