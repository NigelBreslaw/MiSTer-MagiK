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
