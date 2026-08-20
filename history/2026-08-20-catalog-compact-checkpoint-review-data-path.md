# Independent catalog data-path review

Review target: evidence commit `8433aa13c`, qualified code revision
`9fe598fdcddccf7c2ef9525d56b994e46427176d`.

All 14 frozen artifact hashes matched. The retained and rejected decisions are
consistent with the recorded phase data and exact catalog fingerprints.

## Verified findings

- Compact frames removed the dominant bounded checkpoint cost while retaining
  fail-closed frame bounds, hashes, decompression, and publication recovery.
  They are the only performance change supported by a matched parent/candidate
  comparison.
- In the final whole-card legs, scan cost 75.684 s first-clean, 48.355 s
  warm-clean, and 52.450 s rebuild. Projection cost about 21-22 s, shard build
  about 26.5 s, and publication about 25.6-28.3 s. These phases overlap and
  cannot be added as if they were serial.
- Warm-clean scan evidence reports 27.130 s in namespace producers and
  47.264 s in classification, with 1.185 s of channel-send time and 550 slow
  sends. Archive table-of-contents work accounts for 13.783 s.
- Arcade first-visible work contains about 1.535 s of scan and 3.435 s of
  preparation, including 3.036 s of catalog projection. Snapshot creation is
  only 19.8 ms and is not the target.
- Exact replay validates and decodes too much work for too few reusable
  targets: the measured rebuild regressed from 20.413 s to 25.751 s. Changing
  the frame codec did not repair the mechanism. Metadata-only exclusion would
  weaken correctness and remains out of scope.
- SQL multi-row FTS batching increased the C64 row loop from 2.961 s to
  4.133 s. A future search experiment must use a different storage mechanism,
  not another spelling of the rejected statement batch.

## Inferences and next experiments

### Add bounded target-sized candidate batches

The producer/classifier split and slow sends suggest that transferring bounded
classified candidate batches, rather than individual discovery events, may
reduce synchronization and allocation overhead while retaining original target
ordinals and deterministic merge order.

- Conservative whole-operation impact: 5-12 s on full clean and rebuild
  operations; 0.1-0.4 s on Arcade readiness.
- Risks: large batches increase HWM and latency; undersized batches simply move
  overhead; interruption checkpoints must still represent complete targets.
- Experiment: instrument event counts, bytes, queue wait, batch occupancy, and
  per-target consumer time, then test bounded count-and-byte thresholds on real
  Arcade, SNES, and C64.
- Falsify if scan improves by less than 10%, HWM grows beyond 8 MiB, target
  ordering/fingerprints differ, or Arcade regresses by more than 2%.
- Confidence: medium.

### Build a minimal Arcade first-visible projection

Arcade preparation has a measured 3.036 s projection ceiling. Separate the
rows and indexes required for the first visible Arcade page from search and
navigation structures that can safely follow publication.

- Conservative impact: 1.0-2.5 s on Arcade readiness; little change to final
  clean completion unless deferred work overlaps another phase.
- Risks: incomplete initial navigation, generation mismatch, observable result
  reordering, and temporary duplicate state.
- Experiment: first split projection into row, metadata, navigation, and index
  spans. A candidate must preserve the first-page row sequence and final shard
  bytes/semantics.
- Falsify below 0.75 s first-visible improvement, above 3 MiB extra HWM, on
  any output mismatch, or on any unavailable initial interaction.
- Confidence: medium.

### Evaluate a true FTS bulk-build architecture

External-content rebuild or reuse of normalized documents could avoid much of
the per-row SQLite path. This is intentionally distinct from the rejected
64-row prepared statement.

- Conservative impact: 1-2 s on bounded C64-heavy builds and 2-6 s on full
  clean builds; none on unchanged rebuild.
- Risks: row-ID/ranking drift, temporary storage, tokenizer differences, and
  schema migration cost.
- Experiment: prototype outside production publication, compare every query in
  the fixed corpus plus ordered autocomplete output, and retain integrity and
  optimize checks.
- Falsify below 25% search-stage improvement, below 2% affected-operation
  improvement, or on any semantic difference.
- Confidence: low-medium.
