# Scaler off-domain scheduler mailbox design — 2026-08-30

Status: implementation candidate. Certification success is the stop boundary;
this task does not merge, push, publish, install, reboot, or reproduce on the
device.

## Why schema 17 is replaced

Schema 17 captured the right pre-read events, but its 16 sticky bits were
updated inside the dense legacy `Scalaire` process. The fixed-seed build came
within three ALMs of certification, while alternative encodings moved timing,
register, and unconstrained-path results substantially. The diagnostic fanout
was physically perturbing the scheduler it was meant to observe.

Schema 18 keeps the functional scaler and its state transitions unchanged.
`magik_fetch_state[15:0]` is a combinational projection of existing registered
state and gates only. No diagnostic register, reset branch, event update, or
recovery action remains in `ascal.vhd`.

## Capture and CDC contract

The existing 100 MHz Avalon observer retains its reset-retentive accepted-read
scoreboard, return phase, watchdog, immutable publication bank, and host
acknowledgement handshake. A no-request timeout no longer samples the 16-bit
scaler bus asynchronously. It toggles `snapshot_request_toggle` and starts a
second bounded watchdog interval.

`mister_magik_scaler_scheduler_snapshot` receives that request through two
synchronizer stages in the scaler clock domain. It observes one complete
scheduler revolution, from the first `sHSYNC` entry through the next. The live
two-bit output state and existing gates are converted into the schema-17
event-order fields and ORed directly into the held 16-bit bank. State
transitions such as `sHSYNC -> sREAD` and `sREAD -> sWAITREAD` are detected from
adjacent source-clock samples, so a one-source-cycle event cannot be missed.
Failure to complete the revolution leaves the response pending and the
existing destination watchdog publishes `observer_fault`.

At the end of the revolution, the source sets `window_valid` in the already
complete `evidence_hold` bank and only then mirrors the request into
`response_toggle`. `evidence_hold` remains unchanged until a later request.
The response crosses through two destination synchronizer stages before the
100 MHz observer copies the held bus into its existing frozen storage. Explicit
10 ns net-delay constraints bound request, response, and every synthesized
held-data path. There is no false path or timing waiver.

The capture fails closed as `observer_fault` if its response times out, reset
or Avalon progress occurs during the evidence window, burst/return accounting
is malformed, or publication coherence is lost. Such a record cannot be used
for root-cause attribution.

## Evidence layout

The four-word command `0x68` and magic `0x4d58` remain unchanged; the schema is
18 and the architecture is `scaler-off-domain-scheduler-snapshot-v1`. For a
qualified `no_request_seen` record, the state word retains the schema-17 event
layout: window valid, output enable, horizontal edge/start, HSYNC state and
iteration/decision, read versus skip, skip gates, READ/address/request issue,
WAITREAD, and zero vertical size. Host decoding retains schemas 14–17 and
rejects impossible event ordering.

## Pre-synthesis proof obligations

- The exact pinned Menu patch must apply and compile with GHDL.
- Icarus must reconstruct `0x78df` from a repeating sequence in which each
  DISP, HSYNC, READ, and WAITREAD transition lasts one scaler clock.
- Formal proof must show every event present on a capture edge appears in the
  held accumulator or in the acknowledged result, accumulated bits are
  monotonic, and held evidence is immutable between completed captures.
- Existing completion accounting safety, temporal induction, and every
  non-vacuity cover remain mandatory.
- Protocol generation, Rust rollback decoding, Verilator lint/coverage, CDC
  fixture policy, and the pinned integration checker must all pass before the
  typed Apple-container fixed-seed signoff is permitted.
