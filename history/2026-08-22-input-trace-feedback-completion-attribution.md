# Input trace qualification: feedback completion attribution

## Scope

This records the failed exact-device qualification attempts for Main proxy-v3
revision `639d3694e1b93660020e9587cd0fe27f0170ce4c` with the current production
MagiK runtime revision `34b8dfb2e2f02d742a2e6f6ae47367b792a2415c` and host workflow revision
`1737407c2e7707a1738e977dd63ff78315e70121`.

## Checklist

- [x] Reconciled the current committed MagiK Dev delivery.
- [x] Reapplied the exact committed local Main overlay.
- [x] Ran two bounded `input-latency-lab` attempts.
- [x] Preserved production reader scheduling.
- [x] Identified the dominant failed phase before changing code.
- [x] Limited recovery to selection-feedback retirement correctness.

## Results

The first attempt timed out with 55 of 65 required response confirmations,
54 of 64 required feedback removals, and zero outstanding visible feedback.

The reconciled rerun timed out with 62 of 65 required response confirmations,
61 of 64 required feedback removals, and zero outstanding visible feedback.
Its partial trace contained `press_id=64` and proxy sequence 127, proving that
all 64 injected navigation presses reached the production launcher input path.
The benchmark stopped before requesting Main trace flush, so neither attempt
produced an authoritative proxy-v3 summary or overflow result.

The dominant failed phase is therefore launcher presentation and selection
feedback completion after input dispatch, not Main reader capture. The bounded
recovery is to replace destructive surface clearing with receipt-driven
retirement that preserves in-flight visibility and distinguishes never-visible
cancellation from physical removal. Reader scheduling remains unchanged.

## Performance

- Baseline Main revision: `f290719e97f5a3c84efa8e24691b80673b93f23c`.
- Candidate Main revision: `639d3694e1b93660020e9587cd0fe27f0170ce4c`.
- Attempt 1: confirmed 55/65; hidden 54/64; outstanding 0.
- Attempt 2: confirmed 62/65; hidden 61/64; outstanding 0.
- Absolute rerun delta: +7 confirmations and +7 hidden receipts.
- Percentage rerun delta: +12.7% confirmations and +13.0% hidden receipts.
- Acceptance gate: failed; no authoritative trace summary was emitted.
- Regressions checked: exact Dev revisions, all 64 injected presses on the
  reconciled rerun, and unchanged reader policy.
- Attempt paths:
  `build/agent-benchmarks/input-latency-lab/1787355363/` and
  `build/agent-benchmarks/input-latency-lab/1787355534/`.

## Retirement recovery outcome

The bounded recovery was committed as `d35461693ee28369b00cd4869896a3dbc6734dd9`
and qualified with Main `639d3694e1b93660020e9587cd0fe27f0170ce4c`.

- `modal-input` passed, including modal isolation and final held-state release.
- Proxy-v3 `input-integrity` passed idle and forced-stall scenarios with 109 of
  109 initial presses each, final held state false, zero loss or duplication,
  and zero proxy write failures, journal overflows, or sequence gaps.
- The first full `launcher-response` attempt stopped at 2/9 confirmations with
  one visible feedback event still outstanding.
- The bounded rerun completed earlier arms and reached the rotated 71 ms arm.
  That arm stopped at 5/9 confirmations, while all four feedback events that
  became visible received exactly one hidden receipt and left zero outstanding.
- The proxy-v3 latency lab reached 56/65 confirmations and 54/64 hidden
  receipts with one visible event outstanding. `press_id=65`, the final
  injected initial press including the transition press, appeared in the
  partial trace.

The correctness recovery therefore removes the destructive-retirement failure
for every physically confirmed event observed in the bounded rerun. The full
gate remains open because fast-schedule response frames are not all confirmed.
The latency-lab recovery reached more than 80% of both confirmation gates, so
the roadmap permits one further bounded recovery. The next commit is restricted
to reader/mailbox/dispatch attribution; it does not change production reader
scheduling.

Recovery artifacts:

- `build/agent-benchmarks/modal-input/1787356386/summary.json`
- `build/agent-benchmarks/input-integrity/1787356689/summary.json`
- `build/agent-benchmarks/input-latency-lab/1787356409/`
- `build/agent-benchmarks/launcher-response/1787356226/`
- `build/agent-benchmarks/launcher-response/1787356719/`
