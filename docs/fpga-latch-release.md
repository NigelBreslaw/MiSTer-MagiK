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

## Six-hour qualification

Qualification is an attended, identity-locked run of the production renderer,
real catalog worker/scanner, media decoders, and input paths. It repeatedly
forces cold catalog generation while rotating particle animation, transitions,
rapid Arcade scrolling, preview/archive decoding, search/model churn, and
keyboard/controller/pointer traffic.

Run the gate only through:

```text
scripts/agent release qualify
```

The fixed workflow performs its runtime, catalog, and handoff preflights, then
runs `latch-v5-six-hour-stress` before the independent multi-mode display
matrix. The launcher reads the real installed libraries and metadata, but
writes every generated catalog artifact to a volatile isolated qualification
directory. The host rotates the six stress classes every five minutes,
requests serial cold catalog generations every seven minutes and thirty
seconds, and reads the CRC-protected receipt/status interfaces every five
seconds through the same FPGA transaction lock used by production.
The stress phase arms a self-deleting one-shot qualification environment and
then performs one supervised clean boot. The qualification launcher is the
first and only launcher session on that boot; the immutable boot and launcher
session identities are captured afterward. Main ownership, crash, and
invariant counters must therefore remain zero from that boot rather than being
compared with or offset by an earlier baseline.
The attended host process verifies its volatile release token before a
supervised reboot and recreates only that token after reconnecting. The
configuration snapshot is retained on the FAT volume until restoration so a
reboot cannot silently discard the rollback source.

Raw samples and the terminal summary are retained under
`build/release-qualification/latch-v5/<candidate-id>/<run-id>/`. The summary
binds the immutable identity block and the SHA-256 of the NDJSON sample stream.
An interruption or any failed sample fails the gate and still runs the release
restoration path.

The exact candidate must retain at least:

- six hours elapsed and 12 cold catalog generations;
- 1,000,000 accepted and active-confirmed frames;
- 250,000 catalog/UI overlap frames;
- 25,000 frames in every UI stress class;
- 4,000 authoritative receipt/status samples;
- zero forbidden state, rejected production post, ambiguous transaction,
  blocked Main write, crash, hang, black frame, or compatibility screen;
- maximum observed frame wall time of 250 ms;
- maximum consecutive vblank-miss streak of two frames;
- launcher RSS high-water mark no greater than 192 MiB;
- final launcher RSS no more than 32 MiB above the starting RSS.

Evidence is rejected if release, bundle, candidate, manifest, runtime build,
runtime binary, Main, module, RBF, boot, or launcher session identity differs.
Any unknown or mixed report created during the run fails qualification. Any
source or artifact change requires a complete rerun.

Main/RBF changes to the launcher bootstrap additionally require the complete
qualified-black evidence set in `docs/bootstrap-black-qualification.md` before
this six-hour gate. The exact Main/RBF/runtime tuple used for those four
1920x1080 at 30 fps movies must match the six-hour candidate identity; evidence
from a previous latch qualification cannot be carried forward.

After qualification, the identical candidate tuple runs a seven-day canary
with daily concurrent cold-catalog/UI stress. Promotion never rebuilds or
relabels the tuple or its evidence.
