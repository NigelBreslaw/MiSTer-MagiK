# Scaler copy-retirement diagnostic design — 2026-08-24

The installed schema-5 candidate reproduced a genuine persistent MagiK black
at epoch 1, attempt 1. Three coherent samples were identical: flags `0x0541`,
state `0x100a`, `readlev=2`, `copylev=2`. No read acceptance, current return,
nonzero return, or completion activity occurred. Completion request,
destination observation, and acknowledgement agreed; pending, retained return
credits, phase, and drain were all zero. Copy-read, line-write, and raw-active
timing continued, but every corresponding nonzero-data flag was absent. The
authoritative framebuffer remained correct and the physical still was uniform
black.

This rules out the queued completion transport as the immediate stuck state:
there is no queued completion or reset-era return to deliver. It also makes
another broad liveness observer low value. The unresolved choice is narrower:

1. the copy FSM reaches its existing terminal branch but `lev_dec_v` is lost;
2. active copy shifting never satisfies that terminal predicate; or
3. copies retire but stale/incorrect front metadata repeatedly selects zero
   DPRAM contents.

Schema 6, `scaler-copy-retirement-v1`, replaces schema 5. It records the exact
terminal predicate components, terminal-branch and decrement events, copy FSM
occupancy, address wrap, copied-data nonzero, and repeated/changed front
`{prim,last,bank,offset}` signatures. It does not retain the schema-5 Avalon,
line-buffer, or raw RGB observer. The full encoding and conservative host
decision table are in
[`../docs/fpga-raw-scaler-diagnostic.md`](../docs/fpga-raw-scaler-diagnostic.md).

Cycle alignment is explicit: events use the pre-edge registered FSM and
metadata plus current-process variables `hcarry_v` and `lev_dec_v`; the record
closes on the existing output VS edge and publishes the accumulated events
including that edge. State is the registered pre-edge snapshot at closure.
The first copy-start signature is reset at every frame boundary; later starts
set sticky same/different flags.

## Decisive field result and retirement

The schema-6 candidate subsequently passed its local and fixed-seed gates and
was installed experimentally. After 75 valid clean returns it reproduced a
genuine persistent MagiK black. The preserved failure and live records both
classified `scaler_copy_terminal_condition_stall`; three coherent samples were
identical at flags `0x15e1` and state `0x83ea`.

The copy FSM was `sCOPY` with `readlev=2`, `copylev=2`, `o_adturn=1`, front
`prim=1`, front `last=1`, bank `1`, offset `0`, and `o_copyv(0)=1`. Sticky
events proved active copy shifts, next-word phases, line-last activity, and an
address wrap. The bank-terminal term, exact terminal branch, `lev_dec_v`, and
nonzero copied-word event were all absent. The physical output was uniformly
black while the authoritative framebuffer remained correct.

Production inspection then identified the exact missing transition. The last
horizontal-carry edge registers `o_last`, but the legacy shift branch is gated
only by `hcarry_v or o_dshi>0`; it closes on the next edge before `o_last` can
drain through `o_last1/o_last2`. Front `last=1` also disables the alternative
bank terminal, so the block cannot retire.

Schema 6 is now retired rather than stacked. The next candidate is the
functional copy-tail repair documented in
[`../docs/fpga-raw-scaler-diagnostic.md`](../docs/fpga-raw-scaler-diagnostic.md).
It adds `o_last` only to the shift-activity gate and suppresses new pixel-valid
events during those tail-only shifts. The candidate exposes no diagnostic
opcode. The epoch-8 attempt-06 incident remains preserved and unrecovered;
this implementation work performs no device action.
