# Catalog PMU Optimization Campaign

Date: 2026-08-07

## Outcome

The campaign added cross-thread catalog PMU evidence and a safe, isolated
catalog workload. Two evidence-selected optimizations reduced some CPU work,
but neither met the predeclared end-to-end retention gates. Both experiments
were reverted with ordinary commits, so local `main` contains the complete
history.

No catalog performance optimization was retained. The PMU infrastructure was
retained because it identifies CPU-heavy work separately from filesystem and
SQLite wait and rejects changes that improve counters without materially
improving catalog cadence.

## Instrumentation Commits

- `1c11c2a1` — bounded cross-thread PMU profile collection.
- `24aa7b90` — fixed catalog worker, walker, shard, and publisher phases.
- `58db3578` — isolated fresh, incremental, and rebuild-all catalog workload.
- `ebb69d3c` — four-workload PMU suite qualification.
- `df0e4178` — qualification, ratio, experiment, and NEON policy.
- `f5231ad6` — retain the installed revision, GUI SHA-256, and manifest SHA-256
  in subsequent saved PMU suite reports. Earlier suites validated and emitted
  the identity but did not copy it into `summary.json`.

## Baseline

Baseline source commit:
`df0e4178c392d8af32b0cde4c81d6bafc5abd065`.

Both suites used the same exact installed build, sampling interval 1, record
limit 4096, and passed catalog isolation, manifest stability, counter, profile
loss, and correctness checks. The old v2 summary omission means the complete
baseline GUI digest is present only in the workflow result rather than the
saved suite JSON; `f5231ad6` fixes that evidence gap for future and final runs.

Evidence:

- [baseline 1 suite](../../build/agent-benchmarks/pmu-profile/1786059887/summary.json)
  and [catalog report](../../build/agent-benchmarks/pmu-profile/1786059887/catalog.json)
- [baseline 2 suite](../../build/agent-benchmarks/pmu-profile/1786060249/summary.json)
  and [catalog report](../../build/agent-benchmarks/pmu-profile/1786060249/catalog.json)

The two-run median is the arithmetic midpoint for this even-sized sample.

| Operation | Wall (s) | Catalog import (s) | Measured cycles | Instructions | Peak RSS (KiB) |
|---|---:|---:|---:|---:|---:|
| Fresh build | 132.969 | 49.264 | 105,338,101,828 | 65,282,513,317 | 112,486 |
| Incremental rebuild | 70.944 | 16.511 | 50,668,406,988 | 31,784,267,231 | 119,328 |
| Rebuild all | 103.745 | 56.511 | 103,997,379,404 | 62,712,714,111 | 119,328 |

The first fresh run took 152.107 seconds and the second took 113.831 seconds.
That cold-filesystem spread is valid evidence and was not discarded. Rebuild-all
was stable at 103.745 and 103.744 seconds.

## Phase Ranking

PMU spans are nested, so phase counters provide attribution and must not be
summed as independent elapsed work. Median cycle rankings were:

| Rank | Fresh build | Cycles | Incremental rebuild | Cycles | Rebuild all | Cycles |
|---:|---|---:|---|---:|---|---:|
| 1 | `catalog.persist` | 36.859B | `catalog.walk` | 16.633B | `catalog.persist` | 45.194B |
| 2 | `catalog.scan` | 15.124B | `catalog.persist` | 13.234B | `catalog.walk` | 15.481B |
| 3 | `catalog.walk` | 14.339B | `catalog.scan` | 9.903B | `catalog.shard.search-index` | 10.922B |
| 4 | `catalog.prepare` | 11.849B | `catalog.prepare` | 9.898B | `catalog.prepare` | 9.892B |
| 5 | `catalog.shard.search-index` | 11.049B | `catalog.shard.search-index` | 0.438B | `catalog.scan` | 9.859B |
| 6 | `catalog.shard.validate` | 6.877B | `catalog.shard.validate` | 0.325B | `catalog.shard.validate` | 6.879B |

Search-index work was 10.5% of fresh measured cycles and 10.5% of rebuild-all
measured cycles, satisfying the materiality gate for Candidate A. In contrast,
rebuild-all SQLite schema and commit accounted for about 0.50% and 0.26% of
measured cycles. Candidate B was therefore not eligible and was not attempted.

## Candidate A: Reuse Normalized Search Fields

Experiment commit:
`21baf0b34c75ef7694d303e89be4774e6c4f2adb`.

Revert commit:
`c254b90f9352414ba50f4a3a24aed82294a50a81`.

Evidence: [screening suite](../../build/agent-benchmarks/pmu-profile/1786060887/summary.json)
and [catalog report](../../build/agent-benchmarks/pmu-profile/1786060887/catalog.json).

The change reused normalized title, manufacturer, control, and path fields.
Golden tests covered FTS rows, autocomplete rows, Unicode, punctuation,
whitespace, noisy words, paths, and empty metadata. The screening suite passed
all correctness, isolation, profile-loss, and memory checks.

| Metric | Fresh | Incremental | Rebuild all |
|---|---:|---:|---:|
| Operation wall change | -19.9% | -7.8% | **-0.58%** |
| Import wall change | -7.1% | -0.5% | -2.5% |
| Search-index cycle change | -11.1% | -12.5% | -11.4% |
| Search-index instruction change | -12.5% | -13.3% | -12.5% |
| Peak RSS change | +0.3% | +0.3% | +0.7% |

The fresh result is influenced by the recorded cold-baseline spread. The stable
rebuild-all operation improved only 0.58%, below the required 2% operation
gate, despite a real reduction in search CPU work. Search L1D refill ratio also
worsened from about 2.08% to 2.29%, and branch-mispredict ratio rose from about
23.22% to 23.51%. The candidate failed its initial screen; no confirmation runs
were permitted, and the explicit revert followed immediately.

## Candidate C: Enlarge Deferred Shard Page Cache

Experiment commit:
`c5b70e875988d70d63e9dbf7652122967b44f3ef`.

Revert commit:
`0fce034a1c732f65bb4aac364f206f5940a81d15`.

Evidence: [screening suite](../../build/agent-benchmarks/pmu-profile/1786061702/summary.json)
and [catalog report](../../build/agent-benchmarks/pmu-profile/1786061702/catalog.json).

Only deferred shard construction changed from a 2 MiB to an 8 MiB page cache;
immediate durability retained 2 MiB. The screening suite passed correctness,
isolation, profile-loss, and memory checks.

| Metric | Fresh | Incremental | Rebuild all |
|---|---:|---:|---:|
| Operation wall change | -18.0% | -10.2% | **-0.77%** |
| Import wall change | -7.5% | -0.8% | -0.6% |
| Search-index cycle change | -1.7% | -1.3% | **-1.5%** |
| Search-index instruction change | -0.6% | -0.3% | -0.6% |
| Peak RSS change | +0.2% | +0.2% | +0.2% |

The target phase failed the required 5% screen and stable rebuild-all wall time
failed the required 2% operation screen. The candidate was reverted without
confirmation runs.

## CPU, I/O, and SQLite Findings

- Search-index construction is genuine CPU work. Candidate A proved redundant
  normalization accounted for roughly 11% of its cycles, but removing that work
  saved too little whole-operation wall time to retain.
- Fresh `catalog.walk` varied from 20.242B to 8.435B cycles between the two
  valid baseline runs. Together with the 38-second fresh wall spread, this
  identifies filesystem/cache state as an important source of fresh-build
  variance.
- Incremental rebuild is dominated by walking, scanning, persistence, and
  preparation. Its search-index phase is under 1% of measured operation cycles,
  so search-specific work is not an incremental-rebuild priority.
- Deferred shard cache enlargement barely changed search-index counters. The
  current bottleneck is not explained by a 2 MiB SQLite page-cache shortage.
- SQLite schema and commit phases are too small to justify a temp-store or
  durability experiment. No relaxed durability, validation, batching, worker
  growth, or production cache rebuilding was attempted.

Wall time remains the optimization outcome. Counter reductions are supporting
attribution, not a substitute for faster catalog creation or rebuilding.

## NEON Eligibility

No eligible catalog NEON kernel was found, so no NEON or codegen-analysis commit
was created.

The material search work consists of SQLite operations, Unicode `char`
iteration, tokenization, normalization, and mostly short strings. It is not one
pure, contiguous `u8`, `u16`, or `u32` inner loop with representative inputs of
at least four 128-bit vectors and byte-exact scalar/SIMD behavior. Filesystem
walking, SQLite calls, hashing, serde, Unicode iteration, and short strings are
explicitly ineligible. Candidate A also demonstrated that an approximately 11%
search-phase cycle reduction yields less than a 2% stable whole-operation gain,
so the Amdahl gate is not met for a narrower speculative SIMD kernel.

Because no NEON implementation exists, there is no catalog SIMD symbol whose
release binary needs disassembly. Compiler flags alone were not treated as
machine-code evidence.

## Remaining Hot Phases

The remaining material phases are catalog persistence/search-index construction,
filesystem walk/scan, preparation, and shard validation. Their PMU profiles and
per-system reconciliation timings are now available for future hypotheses, but
this campaign does not propose an unsupported optimization for them.
