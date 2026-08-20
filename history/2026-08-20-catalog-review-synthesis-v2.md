# Catalog optimization independent review synthesis v2

Date: 2026-08-20

Frozen evidence commit: `9f7833056`

Qualified runtime: `99f6b36399563ed8b5aef883f93a7ca8921e0402`

Inputs:

- `2026-08-20-catalog-review-measurement-v2.md`
- `2026-08-20-catalog-review-data-path-v2.md`
- `2026-08-20-catalog-review-a9-exfat-v2.md`
- `2026-08-20-catalog-optimization-qualification-v2.json`
- `2026-08-20-catalog-optimization-measurements-v2.tsv`

The reviewers worked independently and received no other review output. This
synthesis was produced only after all three reports were complete.

## Recomputed outcome

All three reviewers independently verified all 14 artifact hashes and the
timing transcription.

| Operation | Prior qualified | V2 | Delta |
| --- | ---: | ---: | ---: |
| Whole-card first clean, harness | 231.120 s | 219.913 s | -11.207 s (-4.85%) |
| Whole-card warm clean, harness | 247.302 s | 165.110 s | -82.192 s (-33.24%) |
| Whole-card unchanged rebuild, harness | 116.592 s | 66.071 s | -50.521 s (-43.33%) |
| First clean, builder | 165.759 s | 140.343 s | -25.415 s (-15.33%) |
| Warm clean, builder | 167.083 s | 123.019 s | -44.064 s (-26.37%) |
| Unchanged rebuild, builder | 98.883 s | 47.694 s | -51.189 s (-51.77%) |
| Whole-card Arcade-ready status | 26.414 s | 8.095 s | -18.319 s (-69.35%) |

Against the pre-campaign observation, first clean changes from 272.112 s to
219.913 s (-19.18%) and rebuild from 169.457 s to 66.071 s (-61.01%).

The unchanged-rebuild improvement is the strongest causal result: the removed
builder work (51.189 s) and wall reduction (50.521 s) agree closely. Clean and
Arcade-ready comparisons are directional because the whole-card evidence is a
single fixed-order sequence across multiple retained changes.

The final bounded real Arcade/SNES/C64 qualification passes three of three
pairs after one warmup: 91.260 s fresh median, 30.248 s changed-rebuild median,
9.227 s fresh Arcade-ready median, exact +1 SNES count per mutation, and
unchanged production registries.

## Update and format impact

This optimization phase does not change the public catalog format relative to
the prior qualified revision `dd6e513`: canonical/build 67/17, manifest v1,
SQLite shard v5, mini-navigation v3, NavPack v2, and registry/state/binding/
scanner-cache/search v1. Users updating from that revision reuse their existing
published catalog without an automatic rescan.

Only disposable builder recovery state changed: `build-progress-v3` and
`target-output-cache-v3` now use internal schema v4 compact frames. Incompatible
or interrupted recovery state is discarded conservatively without making it
catalog authority. Users updating from an older recognized public descriptor
are classified upgrade-required and receive a replacement catalog build; the
last published generation remains the recovery authority, although the current
reader does not expose an incompatible older generation during that build.

## Validated retained mechanisms

Strongly validated:

- Full-audit post-scan unchanged exit.
- Compact, chunked, checksummed checkpoint frames and durable bundle rename.
- Serialized, manifest-last publication and prior-generation recovery.
- Fresh two-worker shard pipeline is exercised with peak in-flight two and
  zero fallbacks.
- Rejected JSON replay, preallocation, `copy_file_range`, parallel runtime
  classification, global autocomplete, and MRA-fusion decisions remain absent
  or non-default as intended.

Validated in source and combined qualification, but not isolated for speed:

- MRA prefix read/four-worker prefetch.
- Persisted-search document pipeline.
- Media-work deferral during catalog build.
- Work-mode epochs, idle burst, and cooperative checkpoints.

Not proven by frozen evidence:

- Actual post-checkpoint CPU0+CPU1 affinity.
- Any exercised `Paused` epoch or scroll parking latency (`park_count=0`).
- Authoritative zero-repeat animation cadence.
- Exact per-game/search/artifact equality; the logical fingerprint covers
  system rows and total count only.

## Current measured ceilings

- Whole-card scan: 46.921 s unchanged rebuild; 50.100-64.997 s clean.
- Fresh projection/reconciliation: 43.251 s.
- Shard build wall: 23.688 s.
- Publication wall: 19.862-21.072 s.
- Copy/hash: 18.852-19.650 s for 73,313,125 bytes.
- Checkpoint persistence: 5.504-6.833 s for 31.47 MB input / 3.29 MB frames.
- Search across all systems: 9.227-9.480 s; C64 fresh 3.698 s.
- Post-scan unchanged decision: only 0.543 s and no longer a target.
- MRA prefetch: about 0.536 s and below the opportunity gate.

Nested and overlapping phase values are attribution, not additive wall time.

## Tier 0: correctness instrumentation prerequisite

Before another optimization is retained, extend qualification identity to
cover canonical per-game IDs, lossless paths, launch contracts, ordering,
search result/rank corpus, autocomplete output, and every published artifact
SHA. Record actual applied affinity, per-core CPU time, work-mode epochs,
checkpoint-to-park latency, and physical repeats specifically inside scripted
interaction windows.

This is a correctness and attribution commit, not a performance claim.

## Tier 1: experiment now

### Pipeline shards from proven system finality

Two independent reviewers converged on overlapping deterministic per-system
projection with the remaining scan. The existing finality markers and shard
pipeline provide a bounded implementation path.

- Conservative whole-card impact: fresh 10-20 s; warm clean 8-15 s;
  changed rebuild 3-8 s; unchanged rebuild approximately zero; Arcade
  bootstrap readiness approximately zero unless full-Arcade finality advances.
- First bounded corpus: real Arcade/SNES/C64 plus PC88, GBA, MegaDrive, NES,
  and GBC before whole-card confirmation.
- Required metrics: contributor-set finality, shard start/end, scan/projection
  overlap, complete wall, queue wait, artifact digests, HWM, and Arcade-ready.
- Reject below 5% whole fresh gain, below 10 s proven overlap, above 5% Arcade
  regression, above the 144,328 KiB HWM gate, or on any exact mismatch.
- Confidence: medium.

## Tier 2: instrument first

### Exact target verification and compact-frame reuse

Two reviewers converged on revisiting per-target reuse only because compact
frames change the old 123 s JSON-open cost. They did not endorse metadata-only
validation, and the earlier replay failure remains authoritative for that
implementation.

- Conservative impact: unchanged/changed rebuild 5-10 s initially; measured
  scan ceiling is much larger but must not be claimed.
- Require content-complete mutation cases: MRA/MGL/prepared data, archives,
  names, case, payload/listing changes, preserved size/time, corruption, and
  interruption.
- Reject if validation plus decode exceeds 80% of current execution, whole gain
  is below 5%, or any mutation is missed.
- Confidence: low-medium.

### Phase-aware checkpoint/publication scheduling

One reviewer proposed this and the data-path review independently validates the
19 s copy/hash ceiling and the failure of copy-only preallocation. It remains
instrument-first rather than promoted.

- Conservative impact: fresh 3-8 s; rebuild/Arcade readiness approximately
  zero.
- Preserve current tmpfs construction, one-entry handoff, serialized exFAT
  publication, durability barriers, and manifest-last authority.
- Reject below 20% publication gain, below 5% whole-fresh gain, or on any
  hash/recovery/HWM regression.
- Confidence: medium.

### Search representation experiments

The immutable per-system search cache and contentless/external-content FTS5
proposals attack different mechanisms and have no two-reviewer convergence.
Measure them independently; do not combine with normalization or autocomplete
changes already tested.

- Conservative impact: changed rebuild 5-12 s for cache reuse; fresh/warm
  2-5 s for contentless FTS; Arcade readiness approximately zero.
- Require canonical query/rank/autocomplete equivalence and exact artifact
  validation.
- Confidence: medium-low.

### Adaptive scan/classifier batching and measured CPU1 use

The scan ceiling is large, but the earlier runtime parallel-classification
policy was rejected and frozen evidence does not prove current CPU1 migration
or scroll parking. Instrument actual affinity and parking before another
parallelism policy.

- Conservative impact if later proven: fresh 5-12 s; rebuild 4-10 s; Arcade
  readiness 0-0.8 s.
- Reject on any interaction-window physical repeat, parking latency above one
  frame, HWM above 150 MiB, or below 5% whole gain.
- Confidence: medium-low.

### Minimal immutable Arcade projection

Only one reviewer proposed this. It targets readiness rather than full build
and must never make a partial generation authoritative.

- Conservative impact: Arcade readiness 1-2.5 s; full completion 0-1 s.
- Reject below 1 s readiness gain, above 3% full-build regression, or on any
  final launch/search/navigation mismatch.
- Confidence: medium-low.

## Rejected or closed

- More MRA readers/fusion: measured ceiling below 1 s.
- Global autocomplete accumulator: 3.45% phase gain, below 5% gate; reverted.
- Previous CPU1/runtime classification activation: fresh gain below gate with
  rebuild and first-visible regressions; reverted.
- Preallocated artifact copy: faster copy/hash but slower publication and
  fresh completion.
- `copy_file_range`: exFAT `EINVAL`.
- JSON/SQLite target-output replay: 123.065 s open plus 38.874 s validation;
  remains rejected.
- Further post-scan decision tuning: 0.543 s ceiling.

## Preserved dissent

- Whole-card clean improvements are not statistically attributable from one
  fixed-order sequence.
- “First visible” is status readiness, not a visual timestamp.
- Bounded interaction passing does not prove zero repeated physical frames.
- HWM is process-stage evidence, not complete system memory pressure.
- No optimization should be retained against the current count-only logical
  fingerprint once exact game/search/artifact identity is available.
