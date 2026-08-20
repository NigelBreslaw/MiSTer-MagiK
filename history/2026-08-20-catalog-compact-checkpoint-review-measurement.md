# Independent measurement review

Review target: evidence commit `8433aa13c`, qualified code revision
`9fe598fdcddccf7c2ef9525d56b994e46427176d`.

The reviewer received the frozen evidence manifest, its listed artifacts, and
the relevant source paths. All 14 listed SHA-256 values matched. This review
separates matched causal evidence from cross-run observations.

## Verified findings

- Compact checkpoint frames are the only retained performance change. In the
  same-boot, three-sample comparison, checkpoint write time fell from
  7,417,846 us to 364,825 us (-95.1%), fresh completion fell from 60.868 s to
  54.232 s (-10.9%), and internal builder completion fell from 51.965 s to
  44.994 s (-13.4%). Rebuild completion was effectively unchanged at 20.618 s
  versus 20.413 s. Internal Arcade readiness moved from 7.216 s to 7.283 s
  (+0.93%).
- Stored target data fell from 11,646,487 raw bytes to 1,241,786 compressed
  bytes (-89.3%). Scan-complete RSS rose by 3,192 KiB, which is within the
  campaign's absolute 8 MiB allowance.
- The final bounded run measured 49.598 s fresh, 15.450 s rebuild, and 8.679 s
  first-visible for 17,859 games. Those values include later cache and run-order
  effects and must not all be attributed to compact frames.
- The final whole-card observation covered 69 systems and 39,791 games with
  exact logical-fingerprint parity. First clean was 223.667 s, warm clean was
  156.219 s, and rebuild was 67.930 s, compared with 266.703 s, 240.747 s, and
  77.599 s at the frozen start. There is one sample per leg, fixed ordering,
  unchanged page-cache policy, a different boot, and variable host polling
  gaps. These figures are qualification observations, not a matched causal
  distribution.
- Exact replay, its bincode correction, 64-row FTS batching, and direct
  on-media construction all failed their operation gates and were explicitly
  reverted or left disabled.
- The whole-card harness records current RSS at phase boundaries rather than
  an authoritative peak working set. UI evidence is marked inapplicable, so it
  cannot support smoothness claims.

## Inferences and next experiments

### Stream or partition oversized checkpoint serialization

Whole-card checkpoint work still costs about 5.92 s on the first clean build
and 11.56 s on the warm clean build. The plausible next step is bounded
streaming or partitioning of large target output, with fewer metadata commits,
without weakening frame hashes or interruption recovery.

- Conservative whole-operation impact: 2-5 s on a clean whole-card build;
  approximately zero on unchanged rebuild and Arcade readiness.
- Risks: partial-frame recovery, append-only dead space, higher peak memory if
  partitioning is poorly bounded, and metadata/frame disagreement.
- Experiment: real Arcade, SNES, and C64 plus the largest whole-card target;
  compare checkpoint encoding, sync, commit, bytes, HWM, and internal builder
  wall time. Inject truncation and interruption between frame sync and metadata
  commit.
- Falsify if checkpoint time improves by less than 15%, internal completion by
  less than 5%, HWM grows beyond 8 MiB, or any recovery/catalog hash differs.
- Confidence: medium-high.

### Instrument a system-finality frontier

Scan, projection, and shard work overlap, but the evidence does not show when a
system becomes immutable enough to build safely. Record that frontier before
attempting earlier shard construction.

- Conservative projected impact if at least 10 s of safe overlap is found:
  4-10 s on clean whole-card builds; none claimed for unchanged rebuild.
- Risks: stale shards, nondeterministic order, extra live data, and competition
  with Arcade publication.
- Experiment gate: predict at least 10 s of non-Arcade overlap before changing
  behavior, then require at least 8% internal completion improvement, under
  10 MiB extra HWM, exact outputs, and no more than 5% Arcade regression.
- Confidence: medium for the ceiling, low for the implementation until
  instrumented.

### Instrument bounded scan/classification parallelism

The full-card scan remains the largest rebuild cost, but current phase data do
not distinguish namespace waiting, archive work, classification, and queue
backpressure well enough to select a safe worker policy.

- Conservative projected impact: 5-12 s on clean and rebuild operations.
- Risks: Cortex-A9 CPU contention, exFAT seek amplification, memory growth,
  nondeterministic merging, and launcher interference.
- Experiment gate: first add per-target producer/classifier/archive/queue
  timing and HWM; a later candidate must cut scan by at least 10%, avoid
  doubling any target's P95, stay within 8 MiB, and preserve exact output.
- Confidence: medium.
