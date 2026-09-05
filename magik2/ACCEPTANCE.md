# First-version correction acceptance — 5 September 2026

The review corrections are implemented on `nigel/magik2-tooling`. This record
supersedes the prototype's earlier acceptance claims. See the [correction
list](docs/corrections.md) and [machine-readable evidence index](docs/acceptance-2026-09-05.json).
Raw logs, metrics, profiles and command results remain under ignored
`build/magik2-results`; the index records their paths and exact artifact identities.
No production app, Main binary, FPGA or platform manifest was changed.

## Delivery timings

Twenty attempts per completed case, nearest-rank p95, including all failures.
The first full matrix completed 80/80 deliveries successfully. After removing
repeated executable hashing from startup, the rerun was stopped at the user's
request during Slint measurements. Original sources and the running probe were
restored successfully; the interrupted invocation correctly exits 130.

| Case | First complete matrix p95 | Final rerun p95 | Target |
|---|---:|---:|---:|
| Unchanged deploy | 954 ms | 990 ms (20/20 passed) | 1,000 ms |
| Changed prebuilt | 5,025 ms | 4,615 ms (20/20 passed) | 5,000 ms |
| Rust edit | 14,101 ms | 12,796 ms (20/20 passed) | 15,000 ms |
| Slint edit | 16,073 ms | Incomplete: two completed samples, then interruption | 15,000 ms |

**The Slint-edit target remains unverified after optimization.** The complete
pre-optimization sample exceeded it. Partial samples are not a replacement for
twenty attempts, and performance warnings never block ordinary deployment.
Both matrices, including the interrupted attempt, are preserved in the index.

Reproduce with `scripts/magik2 acceptance` using the configured device login.
It edits only the disposable probe, retains every attempt, and restores the
original sources and app. Avoid concurrent edits in that checkout during the run.

## Hardware correctness and recovery

`scripts/magik2 acceptance --contracts` passed on `192.168.1.117` in 74.855 seconds:

- A fresh credential cache discovered the existing native token and retained
  both agent and probe PIDs; no compatible-service replacement.
- Invalid upload hashes and superseded starts preserved the running artifact.
- Five watch reconnects each received metrics, logs and a valid RGB565 frame.
- A stalled viewer left status responsive in 621 ms.
- Five independent viewer-on motion windows passed; each had zero physical
  drops and zero latch rejections.
- Dropping attachment after test startup restored the persistent probe.
- Stop confirmed Main's launcher recovery; cleanup restarted the probe.

Viewer-on windows presented 280–285 frames in five seconds, compared with 300
in the earlier viewer-closed windows. This is measurable observation overhead,
not physical latch-drop evidence. Keep viewer state explicit when comparing
benchmarks. The host producer test also stalls a real Unix receiver and confirms
publication stays responsive with a bounded latest-frame slot.

## Tests, measurements and profiling

- 42 focused Python tests, 17 native-agent tests and three probe tests pass.
- Both Rust packages pass focused Clippy with warnings denied. Python formatting
  and basic correctness lint pass; hosted CI has not been run from this task.
- Smoke passed through the pinned Slint Python client with exact build identity.
- The shared pytest runner recorded five independent 2-second warmup/5-second
  measurement windows: 300 presentations each, no physical drops or rejections.
- The separate 10-second instrumented window retained 909 CPU samples, matching
  run and artifact identity, nonempty folded stacks and SVG, and app/renderer
  symbols. This profile preceded the final startup/preview cleanup; it is not
  relabeled as a new final-build profile.
- Latest recovery and viewer-on checks used the final device implementation.
  The user ended further acceptance before another profile repetition.

Run `scripts/magik2 check smoke` or `scripts/magik2 check motion --profile` to
reproduce those scenarios. Both use the same consumer pytest tests; profiling
is an additional labeled repetition, not part of the benchmark distribution.

## Code size

Physical lines including comments, whitespace and embedded tests: Python host
and viewer about 2,200; native agent 2,110; probe 740; host tests about 840;
consumer scenarios 170; scope checker 62. Formatting increased physical line
count without adding a second orchestration engine. No legacy orchestrator
imports, database, release-version gate or rollback ladder was introduced.
