# Passive FPGA HDMI evidence

## Active moving-band campaign: schema 20

The preserved 2026-08-28 recurrence showed a byte-stable framebuffer and
visible moving-band corruption while schema 11 returned CRC-valid but
permanently zero records. That is evidence that schema 11 never completed a
fetch epoch, not proof that the production fetch boundary itself stopped.
Schema 11 used `reset_req` as an asynchronous observer reset even though
accepted Avalon return obligations survive that production reset boundary, so
the observer could erase its own queue, phase, heartbeat, and publication
evidence.

Schema 14 introduced `scaler-fetch-liveness-first-stall-v1` as a four-word
record on read-only command `0x68` with magic `0x4d58`. Schema 15 then replaced
its frozen state word with the external Avalon no-request gates. A preserved
rolling-corruption recurrence produced three coherent schema-15 publications:
normal fetch liveness had occurred, then no external Avalon request appeared;
the Avalon controller was idle, reset was released, and the retained-return
drain was open and empty. That rules out the framebuffer, latch transaction,
Avalon wait/return path, retained reset drain, and completion queue for the
captured failure. The remaining unobserved boundary is the output-clock
`ascal` scheduler which generates a new request.

Schema 16, `scaler-output-scheduler-gates-v1`, keeps the same four-word
`{schema, flags, state, CRC}` command and all schema-15 watchdog, publication,
CRC, and acknowledgement machinery. It replaces only the existing 16-bit
state tap with output state, copy state, read/copy levels, request phase and
acknowledgement state, address readiness, and the exact copy-terminal gates.
The host retains schema-14 and schema-15 rollback decoding. A malformed
acknowledgement or record fails closed and never falls back.

The watchdog observes only the existing top-level `vbuf_address`,
`vbuf_burstcount`, `vbuf_read`, `vbuf_waitrequest`, and
`vbuf_readdatavalid` control wires in `clk_100m`. It has no return-data or
`ascal` output tap. Its two-entry accepted-obligation scoreboard and 0–127
return phase are never cleared by `reset_req`; synchronized reset is recorded
as data, and returned beats continue draining accepted obligations while reset
is asserted. A stable publication bank remains immutable until a complete host
read is acknowledged across the two-stage CDC handshake.

A completed 128-beat request establishes prior normal liveness without keeping
address history on the observed interface. The compact record is
`{schema, flags, state, CRC}`. The high nibble of `flags` is a modulo-16
publication sequence. Before a stall or observer fault, `state` is zero;
afterward it is the frozen `{reserved zero, FIFO depth, return phase, cause}`
tuple, or the scheduler evidence word for a qualified no-request result. Host
decoding fails closed on an impossible tuple and requires three exact modulo-16
advancing publications. The publication bank and completed CRC work register
remain stable under the generation/acknowledgement hold interval. CRC advances
one payload bit per cycle using the same polynomial and record value.

The single progress watchdog prioritizes the oldest accepted return obligation,
then a wait-blocked request, then absence of a request. Its default bound is
`2^24-1` `clk_100m` cycles (about 167.8 ms). Progress on the terminal cycle wins
over timeout. The sticky classifications are deliberately observational:
`no_request_seen`, `accept_blocked`, `first_return_missing`,
`return_incomplete`, and `request_cancelled`. Normal completed-request evidence is
rolling evidence and never prevents a much later first stall from freezing.
Malformed burst shape, unexpected return, FIFO/phase error, reset ambiguity,
or counter ambiguity invalidates attribution. For schema 16, a coherent
`no_request_seen` freeze preserves the exact output-scheduler gates needed to
separate address readiness, request acknowledgement, credit saturation,
copy-start, copy-shift, and copy-terminal retirement stalls without adding a
functional recovery path.

A preserved schema-16 recurrence then produced three coherent advancing
publications classified as `scaler_output_scheduler_state_stuck`. Normal
liveness preceded `no_request_seen`; reset stayed released, and the frozen
word was exactly zero: `sDISP`, `sWAIT`, empty read/copy levels, and no request
phase. The framebuffer remained clean while USB video showed moving horizontal
bands and a corrupt green lower region. Latch posts and flips advanced with
zero drops or rejects. This rules out every schema-16 post-read class but an
instantaneous `sDISP` sample cannot show what happened across the preceding
167.8 ms no-request interval.

Schema 17, `scaler-pre-read-scheduler-evidence-v1`, therefore replaces only
the same 16-bit tap with a sticky request-delimited summary. A synchronized
Avalon acceptance acknowledgement starts the window; subsequent output-clock
events set monotonic bits until the next acknowledgement. At a no-request
freeze the word reports output enable, horizontal-sync edge and start
consumption, `sHSYNC` vertical iteration and decision, read/skip predicates,
`sREAD` address readiness and request issue, `sWAITREAD`, and a zero vertical
size. This separates the missing pre-read boundaries without widening the
port, changing command `0x68`, or adding a functional recovery path. The host
retains schema-14 through schema-16 rollback decoding and rejects impossible
event orderings.

The schema-17 implementation was diagnostically correct but did not qualify:
its best fixed-seed build was three ALMs above the unchanged experimental
resource ceiling. More importantly, all schema-17 variants retained temporal
state inside the physically sensitive `Scalaire` process. That made placement
depend on which diagnostic bits were registered even though they had no
functional fanout.

Schema 19, `scaler-off-domain-scheduler-snapshot-v2`, removes every diagnostic
register and sticky update from `Scalaire`. The existing 16-bit port now
exports only live registered scheduler gates. After the 100 MHz watchdog proves
there has been no external request, it toggles a closed-loop mailbox request.
A separate SystemVerilog observer, clocked by `clk_hdmi`, samples those gates
from one HSYNC entry through the next and reconstructs the same event-order
summary. Because
the observer uses the source clock, it cannot miss a one-cycle scheduler state.
It holds the completed 16-bit word before toggling its response; the two-stage
destination synchronizer therefore qualifies a coherent multi-cycle data path.
If the scaler clock stops, reset or progress interrupts capture, or the mailbox
does not answer within the watchdog bound, attribution fails closed as an
observer fault. Schema 14 through schema 18 remain rollback-decodable.

Schema 19 did not qualify. Quartus preserved the combinational evidence bus as
15 LCELL buffers and replicated the response toggle, so the intended source-to-
destination CDC path was no longer the physical path that the constraints and
checker described. Its rolling publication handshake also kept more CDC state
and local selection logic than a first-failure diagnostic needs.

Schema 20, `scaler-off-domain-scheduler-terminal-v3`, replaces that mailbox
with a one-shot terminal record. The HDMI observer waits for one HSYNC entry,
summarizes exactly the following HSYNC interval into six registered bits, and
then changes a preserved, single-fanout response bit. The 100 MHz destination
captures only those six source registers after the synchronized response; it
expands the compact code locally into the unchanged 16-bit scheduler evidence
format. This removes the combinational CDC payload and makes every constrained
source and destination a real register.

Avalon terminal context and scheduler evidence have separate immutable local
banks. Once the first terminal event is classified, a serial CRC is computed
over one `{schema, flags, state}` value and a one-way `record_ready` bit makes
the completed record visible to the command clock. Command `0x68` remains
unavailable until that bit arrives. The host accepts three CRC-valid identical
schema-20 reads; older schemas retain their advancing-publication rule. There
is no acknowledgement loop, rotating publication bank, diagnostic recovery,
or production-control fanout. Reset/progress interruption and a stopped HDMI
clock still fail closed as observer faults. Schema 14 through schema 19 remain
rollback-decodable.

The wide and staged FPGA video observers are retired from production. Their
field evidence was decisive, but every expanded implementation made the dense
legacy scaler physically sensitive and failed at least one fixed qualification
gate.

The later repair-only two-bit Gray candidate is also not qualified. With every
diagnostic opcode removed it still measured `0.072 ns` setup and `0.190 ns`
hold, below the required `0.428 ns` and `0.200 ns`. It must not be installed or
published.

The repair and qualification policy is documented in
[FPGA scaler return recovery design](fpga-scaler-return-recovery.md). It selects
a queued one-bit request/acknowledgement repair which preserves the legacy
HDMI-side completion cone. Platform-v0.29 kept all observer commands out; the
successor retains the separately qualified schema-10 observer.

The experimental attribution sequence through schema 6 is now retired. Its
last result isolated a production `sCOPY` line-last tail that could stop before
the delayed terminal bit reached `o_last2`, permanently preventing `lev_dec_v`.
The repair-only platform-v0.29 RBF exposes no observer: commands `0x60`
through `0x67` are unsupported. The succeeding diagnostic-enabled platform
candidate adds only the read-only schema-10 `0x67` raw-scaler ordered
signature; latch-v5 and capabilities `0x03ff` remain unchanged. The exact
repair, observer, and proof boundary are in
[Scaler copy-tail repair](fpga-raw-scaler-diagnostic.md). A passing
`experimental_raw_scaler-v1` Quartus result is a valid numbered platform
component and is intentionally installed by ordinary delivery.

`0x67` is diagnostic evidence, not a recovery command. It has no write, clear,
arm, reset, or freeze operation. The host requires three CRC-valid, identical
coherent records before classifying completion backlog, idle credit-accounting
stall, or continuing scheduler progress.

That experiment subsequently reproduced physical black with nine identical
coherent `0x0da3` samples. It ruled out the completion queue and the specified
credit-accounting stall for that occurrence, so the scheduler observer is
retired from the next candidate.

Schema 4 then reproduced persistent physical black with correct framebuffer
content and coherent completed active frames whose raw scaler RGB was exactly
black. That observer is now retired rather than stacked.

Schema 5 then reproduced persistent physical black with full read and copy
levels, an idle completion queue, and continuing copy/line/raw timing whose
data remained zero. That broad pipeline observer is also retired rather than
stacked.

Schema 6 decisively observed the stalled terminal predicate and is not present
in the functional repair candidate. Its generated host decoder remains only
for rollback evidence compatibility.

Schema 7 was a separate disposable experiment for the rare moving-band
corruption, not another black-screen repair. Its four-byte-per-cycle CRC cone
failed fixed-seed timing and resource gates before installation and is retired.

Schema 8 replaces it with one shallow ordered-signature update per qualified
sample at the same direct `ascal` taps and one observer-only isolation stage.
It publishes only the latest signature and advancing sequence through read-only
`0x67`; `0x60`–`0x66`, latch-v5, and capabilities remain unchanged. It has no
final-HDMI observer and continues to report sink visibility as unobserved. See
[the retry design](../history/2026-08-24-raw-scaler-ordered-signature-design.md).

The complete attempt history and retained measurements are in
[FPGA video diagnostics: two attempted designs and their retirement](../history/2026-08-14-fpga-video-diagnostics-design-attempts.md).
