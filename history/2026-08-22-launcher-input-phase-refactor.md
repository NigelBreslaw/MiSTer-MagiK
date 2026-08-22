# Launcher input phase refactor evidence

## Revisions

- Neutral extraction: `bb5c757893d254211cf51f227716910867191997`
- Control-flow recovery: `e9bb74b9a9144978204ae95d9592f8dc9e610bf9`
- Main proxy-v3: `639d3694e1b93660020e9587cd0fe27f0170ce4c`

## Checklist

- [x] Kept the input phase at its original post-housekeeping invocation point.
- [x] Kept drain, batch validation, focus, held/repeat, setup/modal interception,
  automation, navigation, feedback, and state observation inside one boundary.
- [x] Preserved outer launcher recovery continuations with a labeled block.
- [x] Preserved the later batch-empty preemption decisions explicitly.
- [x] Added phase-order and state-parity source contracts.
- [x] Passed the production ARM build and coherent Dev delivery.
- [x] Passed proxy-v3 input integrity and modal input on the exact device.
- [x] Added no selector or scheduling-policy change.

## Results

The first closure-shaped extraction failed the production ARM build because
five existing `continue 'launcher` paths cannot cross a closure boundary and
two later frame decisions still referenced the phase-local input batch. The
bounded follow-up retained one named phase as a labeled block and returned the
batch-empty parity bit. The ARM build and Dev smoke test then passed.

Exact-device parity retained 109 of 109 initial presses in both the idle and
forced-stall scenarios, with zero lost or duplicated actions, proxy write
failures, journal overflows, or sequence gaps. The final held state was false.
Modal input also passed, including isolation during the held input, neutral
state after release, and a fresh press opening Arcade.

## Performance

- Baseline integrity artifact:
  `build/agent-benchmarks/input-integrity/1787356689/summary.json`.
- Candidate integrity artifact:
  `build/agent-benchmarks/input-integrity/1787357312/summary.json`.
- Baseline idle dispatch maximum: 15,236 us.
- Candidate idle dispatch maximum: 15,817 us.
- Absolute idle maximum delta: +581 us.
- Percentage idle maximum delta: +3.81%.
- Baseline idle dispatch p99: 14,243 us.
- Candidate idle dispatch p99: 14,015 us.
- Absolute idle p99 delta: -228 us.
- Percentage idle p99 delta: -1.60%.
- Correctness gate: passed; timing is diagnostic because this commit changes no
  scheduling policy and the integrity workload intentionally includes bursts.
- Baseline modal artifact:
  `build/agent-benchmarks/modal-input/1787356386/summary.json`.
- Candidate modal artifact:
  `build/agent-benchmarks/modal-input/1787357339/summary.json`.
- Modal gate: passed with unchanged terminal state and scanout ownership.
