# Independent Cortex-A9 and exFAT review

Review target: evidence commit `8433aa13c`, qualified code revision
`9fe598fdcddccf7c2ef9525d56b994e46427176d`.

All 14 frozen artifact hashes matched. The review treats traced phase spans as
overlapping evidence and does not infer UI quality from the non-UI benchmark.

## Verified findings

- Runtime policy reserves CPU1 at high priority for UI/input. Catalog and walker
  work run on CPU0 at nice 5 and nice 10; the serialized publisher runs on CPU1
  at nice 10. Any new CPU1 work therefore needs a physical interaction gate.
- The existing build/publish pipeline already reaches two in-flight shards,
  overlaps work by about 18.4-21.0 s, and waits about 7.1 s on the publication
  queue. Adding shard workers without identifying the bottleneck could increase
  exFAT contention rather than reduce wall time.
- Forced generation-private on-media staging cut publication from 4.154 s to
  0.593 s in the bounded comparison, but shard construction rose from 11.075 s
  to 25.051 s and fresh completion from 54.232 s to 62.210 s. Leaving production
  `auto` on tmpfs is the correct decision.
- The full-card artifacts publish about 73.3 MB and spend 25.6-28.3 s in the
  publication phase while maintaining manifest-last durability. That is a
  meaningful optimization ceiling, but not permission to remove hashing,
  synchronization, or recovery barriers.
- Whole-card current RSS is broadly flat, but peak HWM is approximately 136 MiB
  on clean runs and should be a first-class metric in later experiments.

## Inferences and next experiments

### Fuse Arcade discovery with bounded MRA parsing

Arcade namespace discovery and later MRA parsing touch closely related inputs.
A bounded prefix/content parser in the discovery pass may avoid reopening and
rereading files while preserving exact content signatures.

- Conservative impact: 0.5-3.0 s on fresh completion and Arcade readiness.
- Risks: parser duplication, oversized MRA files, error-policy drift, memory
  growth, and loss of exact prepared-profile semantics.
- Experiment: real Arcade only, retain one bounded buffer per active file and
  feed the current parser unchanged. Compare opens, bytes, parse time, HWM,
  fingerprints, and first-visible time.
- Falsify below 10% builder-ready or 0.5 s visible improvement, above 2 MiB
  extra HWM, on any output mismatch, above 5% worst-sample first-visible
  regression, or on any physical interaction regression.
- Confidence: medium-low.

### Test post-reveal cooperative CPU1 classification

After Arcade is published, a low-priority CPU1 helper could consume bounded,
deterministic classification chunks while CPU0 owns ordering and final merge.
This must not add another shard-building or publishing thread.

- Conservative impact: 5-15 s on full fresh and rebuild scans; no claimed
  Arcade first-visible improvement.
- Risks: input/render contention, cache pressure, scheduler latency,
  nondeterministic results, and higher HWM.
- Experiment: activate only after Arcade reveal, pause before each chunk when
  input or publication is pending, and qualify with physical input-to-visible
  latency, repeated/dropped refreshes, affinity, runnable delay, and HWM.
- Falsify on any physical repeat/drop, material input regression, less than 10%
  classification improvement, less than 5% operation improvement, more than
  8 MiB HWM, or any output mismatch.
- Confidence: medium for throughput, low until UI qualification.

### Reduce tmpfs-to-exFAT publication amplification

Keep fast SQLite construction on tmpfs, then test preallocation, phase-aware
copy scheduling, and kernel-assisted copy while preserving validated hashes,
the directory durability barrier, and manifest-last publication. Do not repeat
direct exFAT SQLite construction.

- Conservative impact: 3-10 s on clean whole-card builds; none on unchanged
  rebuild or Arcade readiness if scheduled after the first shard.
- Risks: exFAT allocation behavior, reduced build/copy overlap, checksum
  semantics, and recovery regression.
- Experiment: add per-artifact build, read, copy, hash, sync, and block-I/O
  evidence; compare current streaming copy with preallocated and phase-aware
  variants. Retain the previous generation through injected failures.
- Falsify below 20% publication improvement, below 5% clean-operation
  improvement, on any hash/recovery mismatch, or if shard construction grows
  enough to erase the win.
- Confidence: medium.
