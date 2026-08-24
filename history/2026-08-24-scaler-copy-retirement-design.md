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

The preserved schema-5 incident remains unrecovered. This design work performs
no device action. Local proof is intentionally close to commercial standards,
but synthesis, Apple signoff, installation, and the next physical attempt are
separate attended gates.
