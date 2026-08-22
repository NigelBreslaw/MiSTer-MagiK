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

## Proxy-v3 authority recovery

The host recovery commits `550087a86`, `137b02fa3`, `4ef8155f3`, and
`61e8ee7eb` made incomplete response arms diagnostic without weakening any
product gate. The completed exact-device run used installed MagiK
`e9bb74b9a9144978204ae95d9592f8dc9e610bf9`, Main
`639d3694e1b93660020e9587cd0fe27f0170ce4c`, and host `61e8ee7eb`.

### Authority checklist

- [x] Completed all nine fixed arms in their declared order.
- [x] Retained one driver, partial launcher trace, and Main trace per arm.
- [x] Required exactly 128 contiguous proxy records per arm.
- [x] Required ordered kernel, poll, read, map, journal, and write timestamps.
- [x] Observed zero trace-ring drops and zero proxy `EAGAIN` records.
- [x] Observed zero proxy write failures, journal overflows, and sequence gaps.
- [x] Verified the requested reader affinity, nice value, and scheduler report.
- [x] Preserved failed launcher completion and product-quality status.
- [x] Restored the launcher and original display mode through the canonical runner.

Every arm captured all 64 timed presses and releases in Main. The launcher
confirmed only 56--61 of the 64 timed presses, depending on the arm. This is
why input-integrity and first-eligible-vblank remain failed in the summary even
though Main trace authority passed. No reader-policy winner can be retained
from this run.

### Pre-dispatch attribution

The following figures use only timed presses with a confirmed launcher record.
They are diagnostic subsets, not product-pass samples.

| Arm | Reader capture p95 / max | Capture to publish p95 / max | Publish to drain p95 / max | Drain to dispatch p95 / max | Kernel to dispatch p95 / max |
| --- | ---: | ---: | ---: | ---: | ---: |
| Baseline, current | 238us / 287us | 11us / 13us | 356us / 450us | 121us / 154us | 670us / 835us |
| Forced catalog, current | 3,066us / 27,060us | 25us / 30us | 16,007us / 35,303us | 153us / 182us | 25,702us / 35,766us |
| Forced catalog, CPU1 nice | 796us / 12,693us | 17us / 30us | 12,202us / 16,707us | 168us / 230us | 13,572us / 16,955us |
| Forced catalog, CPU0 RR | 159us / 89,539us | 30us / 114us | 29,743us / 99,893us | 178us / 229us | 41,837us / 100,109us |
| Forced catalog, CPU1 RR | 100us / 3,121us | 34us / 40us | 4,457us / 9,383us | 152us / 193us | 4,610us / 9,678us |

Against the prior idle current-policy capture of 223us p95 and 291us maximum,
the fresh baseline is +15us (+6.7%) at p95 and -4us (-1.4%) at maximum. Against
the prior CPU1 RR result of 84us p95 and 134us maximum, the forced-catalog arm
is +16us (+19.0%) at p95 and +2,987us (+2,229.1%) at maximum. The large maximum
and the incomplete response record reject a scheduling-policy change.

The dominant forced-catalog pre-dispatch phase is publication to launcher
drain, not drain to dispatch. The worst current-policy forced-catalog sample is
proxy sequence 2333 / press 22 at 35,303us publication-to-drain and 35,766us
kernel-to-dispatch. CPU1 RR reduces that phase, but proxy sequence 3221 / press
11 still reaches 9,383us publication-to-drain and 9,678us kernel-to-dispatch.
This directly selects the early-routing experiment while keeping reader
scheduling unchanged.

Main-side maxima remain separately attributable. In the idle baseline,
sequence 2225 owns the 201,297us kernel-to-poll maximum and sequence 2161 owns
the 9,117us read-to-map maximum. These are not conflated with the downstream
proxy-reader capture or mailbox stages.

### Completion outcome

- Baseline: 61/65 total confirmations, 60/64 hidden, zero outstanding.
- Forced catalog current: 57/65 confirmations, 56/64 hidden, zero outstanding.
- Monolithic 16ms: 58/65 confirmations, 56/64 hidden, one outstanding.
- Monolithic 64ms: 62/65 confirmations, 61/64 hidden, zero outstanding.
- Cooperative 2ms: 60/65 confirmations, 59/64 hidden, zero outstanding.
- Cooperative 1ms: 60/65 confirmations, 58/64 hidden, one outstanding.
- Forced catalog CPU1 nice: 60/65 confirmations, 59/64 hidden, zero outstanding.
- Forced catalog CPU0 RR: 62/65 confirmations, 61/64 hidden, zero outstanding.
- Forced catalog CPU1 RR: 61/65 confirmations, 60/64 hidden, zero outstanding.

Artifact: `build/agent-benchmarks/input-latency-lab/1787358833/summary.json`.

## Early-routing experiment rejection

The benchmark-selected early-routing experiment was implemented at
`9522a3dcc`, authorized only by the consumed volatile laboratory token at
`a4b544741`, and given one bounded latch-wait recovery at `837dd70d3` with the
target-lifetime correction at `98d551994`. Production remained on the current
route for every unarmed launch.

### Rejection checklist

- [x] Captured a fresh adjacent current-policy control on the exact device.
- [x] Captured the token-authorized early-routing candidate on the same build.
- [x] Preserved 128 contiguous proxy-v3 Main records with zero trace drops.
- [x] Identified the first failed candidate's dominant delay before the next loop.
- [x] Attempted one bounded recovery during an interruptible latch wait.
- [x] Reused the existing input phase and resumed the same physical receipt path.
- [x] Compared publication, dispatch, confirmation, feedback, and vblank evidence.
- [x] Rejected the candidate without changing production reader scheduling.

The first authorized candidate run reduced neither the tail nor completion:
publication-to-dispatch was 9,110us p95 / 100,390us maximum, with 56/65
confirmations and 55/64 hidden receipts, versus the adjacent current control's
1,188us p95 / 40,447us maximum, 63/65 confirmations, and 62/64 hidden receipts.
Its records attributed the delay to input becoming observable while the prior
response/presentation remained in flight.

The latch-wait recovery improved the candidate publication-to-dispatch p95
from 9,110us to 1,571us (-7,539us, -82.8%), but it did not meet the 1,000us
gate and introduced an unacceptable tail. Against its fresh adjacent control:

| Metric | Current control | Recovered early route | Absolute / percentage delta |
| --- | ---: | ---: | ---: |
| Reader capture p95 / max | 256us / 286us | 665us / 2,689us | +409us (+159.8%) / +2,403us (+840.2%) |
| Publication to dispatch p95 / max | 3,660us / 4,063us | 1,571us / 29,080us | -2,089us (-57.1%) / +25,017us (+615.7%) |
| Kernel to dispatch p95 / max | 3,886us / 4,262us | 3,472us / 29,327us | -414us (-10.7%) / +25,065us (+588.1%) |
| Dispatch to confirmation p95 / max | 35,883us / 59,041us | 39,345us / 1,838,172us | +3,462us (+9.6%) / +1,779,131us (+3,013.4%) |
| Confirmed / hidden | 61/65 / 60/64 | 58/65 / 57/64 | -3 (-4.9%) / -3 (-5.0%) |

All 58 confirmed candidate records used their first eligible vblank, but the
candidate missed the publication-to-dispatch p95 and maximum gates, the
kernel-to-dispatch p95 and maximum gates, and the response-completion gate.
The 1.838s confirmation outlier moves the dominant failure beyond the bounded
pre-dispatch hypothesis. A second recovery would therefore combine a new
presentation hypothesis with this rejected routing change and is not
permitted. The selector and all early/latch routing branches are removed.

Artifacts:

- First authorized comparison:
  `build/agent-benchmarks/input-latency-lab/1787363077/`
- Latch-wait recovery comparison:
  `build/agent-benchmarks/input-latency-lab/1787363987/`
