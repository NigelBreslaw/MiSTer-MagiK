# Independent catalog data-path review v2

Frozen evidence commit: `9f7833056`

Qualified runtime: `99f6b36399563ed8b5aef883f93a7ca8921e0402`

Role: catalog algorithm/data-path reviewer. This review was performed
read-only and without access to the other reviews.

## Verified phase facts

All 14 frozen artifact hashes match.

- Fresh scan: 64.997 s; execution pipeline 61.258 s.
- Fresh projection: 43.251 s, containing 23.688 s shard build and 19.862 s
  publication with 13.231 s overlap.
- Warm scan: 50.100 s; projection 41.980 s, containing 23.581 s build and
  21.072 s publication with 14.847 s overlap.
- Forced unchanged rebuild scan: 46.921 s; post-scan decision: 0.543 s; no
  projection or persist path runs.
- Publication copies and hashes 73,313,125 bytes. Fresh SQLite accounts for
  56,045,568 bytes/13.169 s, NavPack 13,912,683 bytes/3.361 s, and navigation
  3,354,874 bytes/2.323 s. The source is
  `shard_registry.rs::copy_staged_artifact`.
- Search across 69 systems totals 9.480 s fresh and 9.227 s warm. C64 alone is
  3.698 s fresh. The source is `persisted_search.rs::populate`.
- Fresh checkpoints compress 31.47 MB to 3.29 MB and cost 6.833 s; warm costs
  5.504 s. The sources are `library_indexer::flush_target_checkpoints` and
  `BuildProgressJournal::checkpoint_targets`.
- Arcade's first static target is ready at 1.497 s fresh, but its last
  contributor is only proven final at 36.892 s. Any early projection must use
  contributor-set finality, not first-target completion.

## Retained mechanism validation

- Preview availability canonicalization and redundant-publication avoidance
  remain in the projection path.
- Unique target-cache ownership and same-parent durable-cache rename remain in
  `build_progress`.
- The reusable-target second-walk skip remains available but is not exercised
  in v2 (`resume_reused=0`).
- Normalized search reuse and deterministic hash accumulation remain in
  `persisted_search`; the bounded 256-row producer/capacity-one channel is
  ordered by original row ID and gated at both lanes.
- The two-slot fresh shard pipeline is exercised with two workers, peak
  in-flight two, and zero fallbacks. Publication remains serialized and
  manifest-last.
- The on-media prepared-artifact rename route remains available but current
  whole-card qualification exercises external tmpfs staging and copy.
- The no-op rebuild completion route and slow-replay disablement are directly
  exercised.
- Prefix-limited MRA parsing and four-worker prefetch remain, but the measured
  0.465-0.629 s phase is below the opportunity gate.
- The full-audit post-scan unchanged exit is directly exercised.
- Compact streamed checkpoint frames are exercised in 15 batches and 3.29 MB.
- Epoch work gating and the idle two-core policy remain in source.
- Parallel runtime classification and the global autocomplete accumulator are
  correctly absent through explicit reverts.

Instrumentation and benchmark commits must not be credited with speedups.

## Negative ledger validation

The preallocated-copy, `copy_file_range`, parallel-classification, global
autocomplete, and MRA-fusion decisions are all supported by their evidence.
The failed zero-telemetry run published no result and the one retry is valid.
The old JSON target-output replay remains rejected; compact-frame replay is a
new hypothesis, not proof that the old mechanism is now beneficial.

## Top ideas: instrument first

### 1. Pipeline shards from proven system finality

Begin bounded shard/search construction only when the complete contributor set
for that system is closed.

- Conservative impact: fresh 10-20 s; warm clean 8-15 s; unchanged rebuild
  approximately zero; bootstrap Arcade visibility likely unchanged.
- Start with real Arcade/SNES/C64; expand to dynamic/runtime and multi-target
  systems before whole-card qualification.
- Measure finality time, shard-build start, overlap, complete wall, Arcade
  readiness, artifact digests, and HWM.
- Falsify below 5% whole-operation gain, above 5% Arcade regression, above the
  144,328 KiB HWM gate, or on any exact catalog/artifact mismatch.
- Risks: late contributors, generation ordering, RAM overlap, and progress
  ordering.
- Confidence: medium.

### 2. Re-test target replay with compact frames and exact audit

The 3.29 MB frame bundle makes the old 123 s JSON-open result obsolete as a
cost estimate, but no metadata-only validation is acceptable.

- Conservative impact if validation is cheap: rebuild 5-10 s; fresh/warm 0-5
  s; Arcade readiness unchanged.
- Use Arcade, C64, SNES, GBA/NES, and PC88/archive-prepared inputs. Mutate MRA
  content, ROM/listing/archive inputs, names, and timestamps independently.
- Measure frame open/decode, exact validation, execution/classification, and
  whole rebuild.
- Falsify on any missed content mutation, if validation plus decode exceeds
  80% of current execution, or below 5% whole-operation gain.
- Confidence: low-medium.

### 3. Prototype contentless/external-content FTS5 shards

Queries require row ID and BM25 rather than stored FTS text, so a contentless
design may reduce duplicate strings, SQLite bytes, copy/hash, and construction.

- Conservative impact: fresh/warm 2-5 s; unchanged rebuild and Arcade
  readiness approximately zero.
- Use real Arcade/C64/SNES with a canonical query/autocomplete corpus, reader
  compatibility, integrity checks, artifact sizes, then whole-card.
- Falsify below 2% whole-operation gain, below 10% SQLite byte reduction, or
  on any rank/autocomplete/reader mismatch.
- Confidence: medium.

## Dissent

The bounded interaction verdict is not authoritative smooth-animation proof:
the raw physical evidence contains repeated vblanks. Whole-card legs are single
fixed-order observations, and no safe metadata-only unchanged shortcut is
supported.
