# Mini workloads

The benchmark command runs an installed Mini workload directly. It does not
start a Slint session or wait for framebuffer readiness.

```
scripts/magik bench blend
scripts/magik bench blend --visual
scripts/magik bench blend --counters neon
scripts/magik bench blend --counters memory
```

Bare `bench` means `bench blend`. Timing runs exactly twice. Each explicit PMU
command runs once. Visual mode runs one pass and resumes Main automatically;
it cannot be combined with counters. No command automatically retries a workload.

The host reuses compatible services, unchanged builds, and installed artifact
hashes. It records build, upload, execution, and total durations in the result
bundle. A service update requires the `run-benchmark-v2` capability, not a matching
build identifier. Adding a workload to Mini does not require a service update.

The native service launches only its installed Mini executable with
`--bench WORKLOAD --mode MODE`. Workload names begin with a lowercase letter,
contain lowercase letters, digits or hyphens, and are at most 48 bytes. Mini owns
recognition and reports unknown workloads. There is no shell or arbitrary path.
Modes are `timing`, `visual`, `pmu-neon`, and `pmu-memory`.

Execution and pipe draining share a 30-second deadline, or 90 seconds for visual
mode. Client disconnect, timeout, and output overflow terminate the process
group. stdout is bounded to 256 KiB and stderr to 8 KiB; cleanup has a bounded
reap interval. Main restoration is attempted on every execution outcome. Host
request deadlines additionally allow Main handoff/restoration. Uncertain transport
outcomes are reported without rerunning the workload.

## Results

`benchmark-raw.json` preserves received bytes, including invalid/truncated output.
`benchmark-process.json` records exit status, artifact identity, stderr, execution
errors, and Main restoration. Validated `benchmark.json` adds host provenance.
Build/upload phase evidence and source dirty status are in `run.json`.

Schema version 1 contains workload, mode, artifact SHA-256, correctness, fixture
identity and structured metadata, positive work count, samples, and optional
visual summary. Each sample identifies its repetition, fixture and work count,
and includes elapsed nanoseconds and normalized time. Visual summaries contain
presentation statistics, never kernel timing samples. PMU records remain inside
their single diagnostic sample and are prohibited in unprofiled results.

The host validates identity, sample counts, normalized timings and presentation
success. PMU validation requires complete counters and equal nonzero enabled and
running times. Missing counters are errors, not zeroes. Measured zero refills are
valid. NEON clock-enabled cycles do not measure utilization; L1 refills are not
DRAM misses. Event 0x61 is data-cache-dependent stalls despite its legacy key.

`magik.benchmark_compare.compare` compares two loaded result objects offline.
It requires matching workload, mode, fixture, work count, target/build flags and
device, with clean source provenance. It never launches a process. Two timing
samples are a practical comparison, not statistical proof. PMU comparison is
one diagnostic pass per version; environment differences are reported.

## Adding a workload

Implement Mini's preparation, correctness check, and execution; register its name
in Mini's dispatch and return the existing result contract. Reuse the command,
transport and result validator. Keep input preparation outside timing, perform
correctness first, and make profiling explicit. Do not add host/service dispatch
branches, UI controls, automatic matrices, or a campaign engine.

A focused local oracle test and one bounded invocation should establish a new
workload. Stop and discuss inconsistent or slow results instead of tuning in a loop.
