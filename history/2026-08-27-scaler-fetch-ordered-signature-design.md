# Scaler fetch ordered-signature diagnostic — 2026-08-27

## Result

Diagnostic 1 of the persistent moving-band campaign is implemented and has
passed the canonical fixed-seed local FPGA signoff. The accepted builder commit
is `19b2c040cf3cf9f290f0d61a62c09f5487812fee`. It replaces the schema-10 raw
scaler observer with schema 11, architecture
`scaler-fetch-ordered-signature-v1`; schema-10 decoding remains available to
interpret the preserved rollback evidence.

This is a signed diagnostic candidate, not physical evidence and not root
cause. No RBF was installed or activated, and no reboot, reload, restart,
recovery, delivery, or other MiSTer mutation occurred during implementation or
signoff. The preserved moving-band incident therefore remains
`captured-unrecovered` under the receipt in
[`2026-08-27-moving-band-raw-scaler-result.md`](2026-08-27-moving-band-raw-scaler-result.md).

## Narrow observer contract

The disposable observer consumes only the existing top-level `vbuf_*` Avalon
wires in `clk_100m`; it adds no `ascal` port or internal scaler fanout. A
two-entry FIFO mirrors the production maximum of two accepted reads and binds
each accepted 128-beat burst to its returned data. Address wraps delimit fetch
epochs. Each accepted address contributes a tagged four-bit fold to one 16-bit
ordered signature, and each returned 128-bit beat contributes a lane-sensitive
16-bit fold. The four-bit address fold is intentionally only a screening token:
a stable schema-11 signature cannot exonerate fetch scheduling and selects the
later internal split rather than a root-cause claim.

Malformed burst count, queue overflow, unexpected return, return phase error,
overlapping epoch marker, counter overflow, and reset ambiguity fail closed.
Fault publication has an independent sticky source level and two-stage
synchronizer. It aborts an older valid command response until the coherent
invalid record is captured, so a valid-generation toggle followed immediately
by a fault cannot cancel and leave stale valid evidence readable.
Command `0x67`, magic `0x4d57`, and the five response words are preserved. The
words contain schema, valid/fault flags, advancing capture sequence, ordered
signature, and CRC-16/CCITT-FALSE. The generated SystemVerilog and Rust
constants remain owned by the canonical protocol JSON.

## Assurance

The exact pinned Menu source is
`3c3634c0105d78f27aeba66b38966c50dbc42c9b`. Icarus/GHDL simulation passed the
reference signature and CRC checks, stable/change cases, lane permutation,
address-token change, empty/pre-alignment handling, sequence wrap, reset with
an outstanding request and stale return, every fault class, simultaneous FIFO
pop/push, immutable command reads, and valid publication followed at the next
source edge by a fault. All 47 delta-checker fixtures passed. Structural checks
passed observer-only fanout, exact top-level binding, retired-tap absence, and
unchanged production integration.

The unchanged completion and copy-tail suite passed BMC, covers, and induction
without new assumptions at exact root
`19b2c040cf3cf9f290f0d61a62c09f5487812fee`. Its production patch SHA-256 is
`612b0378442aa08a04e6a620646598d4e9e13305ce8be71851d0e4ef227da8c2` and the
patched `ascal` SHA-256 is
`70846aacfc77c069dd26f694bf45d6c4359af70e8fb80ba21f87aa54d17c4d5b`.

Several fixed-seed candidates were rejected without seed sweeps, waivers,
placement directives, fitter changes, or gate changes. The closest earlier
candidate used 214 ALMs over baseline against the hard 208 ceiling. A compact
CRC phase experiment regressed to 242 ALMs and was rejected. The first
four-bit-address candidate reached 217 ALMs and was rejected. Only
observer-local folding and CRC elaboration logic changed between these
candidates; production RTL and all fixed gates remained unchanged.

Final review then found that a valid toggle and immediately following fault
toggle could cancel before `clk_sys` observed either edge. The pre-review RBF
was withdrawn without installation. The first sticky-fault fix passed resource,
warning, CDC, and MTBF gates but was rejected at `0.232 ns` setup slack. Removing
only an unnecessary placement restriction from the single-destination fault
source recovered timing; neither seed nor fitter policy changed.

## Accepted fixed-seed signoff

Canonical `scripts/agent fpga signoff` used Quartus 17.0.0 Build 595, seed 2,
and profile `experimental_scaler_fetch-v1`. The retained delta report is
`valid=1`, `invalid_reason=ok`:

- setup slack `0.456 ns`, hold slack `0.249 ns`, and zero TNS;
- 204 ALMs and 156 registers over the matched baseline, within the hard limits
  of 208 and 224;
- unchanged block RAM, DSP, PLL identities, and accepted warning identities;
- exact completion-request, completion-ack, scaler-fetch generation, and
  sticky scaler-fetch fault CDC paths, with combined custom-chain MTBF
  `249999999.99999997` years, above
  `10^12` device-hours;
- 158 baseline and 158 patched unconstrained output paths, with no diagnostic
  unconstrained-path exception.

Artifact SHA-256 values:

- RBF: `f0c80706681cdc3126a0bd26dc089b0f4576a0ffba42f3a5088420feb1dce60e`;
- metadata: `c915b0566443f165554e47f9f79b8fe1c6bad0de34b26ec2866c6da6207d4d6d`;
- delta signoff report:
  `a5c9cf0c47b4ae4527995c45eb0e78e620eac02f96e7a3ba092e328d961a8c51`;
- local signoff input:
  `650329259ebe5e7c087004624d619212cd11733cc2fbaae75464a52dfc22c1e8`;
- build log: `1537874ed772f05d17dc1d77b040eeeb727d685459fb2a64ee86e13cff6f6974`;
- top timing log:
  `73963c41f960305962b418a5355e064a1b7813657d4c3c4dc3cfc19042b3c668`.

Large artifacts remain ignored under `build/fpga-local-apple/signoff/`.

## Next attended decision

Installation remains a later explicit attended operation through the
rollback-capable Dev experimental transaction using the exact RBF, metadata,
and signoff report above. After decoder deployment and three advancing smoke
records, a recurrence requires continuous native USB video, byte-stable source
captures before/during/after, route and latch identity, and at least three valid
schema-11 records. A changing fetch signature selects the address/data split;
a stable signature selects the HSCAL/direct-output split. Neither result alone
is the final root cause.
