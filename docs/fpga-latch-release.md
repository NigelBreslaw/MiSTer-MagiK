# FPGA latch-v5 production requirements

Latch v5 is the only production protocol. New builds must not contain v2/v3/v4
negotiation, decoding, fixtures, feature switches, fallback presentation, or
rollback paths. Main, the scanout module, the latch RBF, and the MagiK runtime
are one platform candidate and are installed or rejected as one transaction.

## State invariant

For an accepted sequence `N`, only these states are legal:

- `accepted=N`, `active=N`, `pending=false`; or
- `accepted=N`, `active=N-1`, `pending=true`, `pending_sequence=N`.

`accepted=N`, `active=N-1`, `pending=false` is forbidden. An interrupted SET
is a rejected attempt; it does not advance accepted state and must never be
described as posted.

Every SET start advances the 16-bit FPGA transaction identifier. Software
extends transaction and sequence wrap into 64-bit journal counters. Every
attempt has exactly one CRC-protected terminal receipt. A post is accepted by
software only when the receipt identifies the attempted transaction and
sequence, accepted state advances once, pending or active names that same
transaction, and active confirmation reaches it while the FPGA lock is held.
An ambiguous SET is never retried.

## Wire and ownership requirements

- Protocol is exactly `5`; capabilities are exactly `0x03ff`.
- SET, status, diagnostics, capabilities, receipt, and presentation telemetry messages are
  CRC-16/CCITT-FALSE protected.
- Presentation telemetry atomically counts every MagiK-owned vblank as either
  a new presentation or a repeat; ownership loss invalidates cadence evidence.
- The `0x5c` telemetry command returns ten snapshot words followed by CRC:
  wrapping 32-bit owned, presented, repeated, and ownership-loss counters,
  then active sequence and live status flags. Its acknowledgement magic is
  `0x4d4c`.
- The command-start edge snapshots pre-event registers. Subtracting two
  snapshots therefore measures the half-open interval from the start command
  edge through, but excluding, the end command edge: a coincident start vblank
  is included and a coincident end vblank is excluded.
- A legacy write while MagiK owns scanout increments ownership loss. A
  same-edge legacy write wins over apply and that vblank is classified as
  neither a presentation nor a repeat. Unowned vblanks are never counted.
- A second SET while presentation is pending is rejected and cannot replace the
  pending frame.
- Active base, geometry, sequence, and transaction become visible atomically;
  pending clears only on that successful edge.
- Main owns every FPGA SPI/GPO writer until it transfers an ownership epoch to
  MagiK. Cross-owner writes are blocked and counted.
- Main recovers ownership before handoff, shutdown, restart, or terminal
  recovery.
- Runtime failure freezes the last confirmed frame. There is no fb0 fallback,
  black compatibility route, or compatibility display popup.

## Diagnostic identity

Every current report uses `mister-magik-latch-failure-report-v2` and includes:

- runtime version, build number, source revision/dirty state, and binary hash;
- platform release number/tag, bundle ID, candidate ID, and manifest hash;
- Main, scanout-module, and latch-RBF hashes and source revisions;
- protocol/capability identity;
- device boot ID and launcher session ID;
- classification and any validation failure.

Reports live under an exact release/bundle/build/binary/boot/session namespace.
Only the namespace’s own `latest.json` is current. The root
`current-identity.json` is a pointer containing the same identity; readers
reject a pointed report whose identity differs. Reports without v2 identity
are `legacy-unidentified`; internally inconsistent identities are
`mixed-invalid`. Neither is health or qualification evidence.

## Deterministic gates

Simulation must include named reproductions for `1213/1212/no-pending` and
`962/961/no-pending`, sequence and transaction wrap, interruption after every
SET word, every vblank phase, reads on the apply edge, suppressed apply,
concurrent Main/runtime/agent access, injected illegal pending clearing, and
seeded randomized interleavings. Assertions must make the forbidden invariant,
unreceipted active state, and rejected-attempt mutation fatal.
