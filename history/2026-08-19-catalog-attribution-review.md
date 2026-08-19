# Catalog build and rebuild attribution review

Date: 2026-08-19

Installed runtime: `4eb3280730c55990e8fb1d126fc83f9064069223`

Evidence index: `history/2026-08-19-catalog-attribution-evidence.json`

This review was performed by the primary agent only, following the explicit
instruction not to use sub-agents. Recommendations are separate from the
captured evidence. The control arm is the only timing authority; profiled arms
are used only for attribution.

## Workload and result

Every leg used the real Arcade, SNES, and C64 folders and published to isolated
exFAT output. All control, PMU, storage, and Streamline legs produced the same
17,859-game catalog: Arcade 968, C64 15,089, and SNES 1,802.

| Control measure | Run 1 | Run 2 | Run 3 | Median |
|---|---:|---:|---:|---:|
| Fresh first visible | 23.738 s | 10.010 s | 9.938 s | 10.010 s |
| Fresh durable complete | 79.980 s | 68.257 s | 67.641 s | 68.257 s |
| Rebuild first visible | 2.238 s | 2.175 s | 2.157 s | 2.175 s |
| Rebuild durable complete | 49.942 s | 48.328 s | 48.762 s | 48.762 s |

The first fresh run is a real cold outlier. The later fresh runs preserve the
previous Arcade-readiness win at approximately ten seconds for this larger
three-system workload; this campaign did not exercise scrolling, so it does
not qualify UI smoothness.

## What the new instrumentation changed our understanding

1. Resume fingerprint validation is not the rebuild penalty. The rebuild had
   committed state present, but zero committed/reused targets and only 2–17 us
   of validation. It still walked 38,790 files and spent 19.64–19.86 s in the
   scan. The earlier durable-namespace-snapshot hypothesis is therefore not the
   first experiment for this path.

2. The scan producer is not the whole scan cost. In the representative rebuild,
   directory production took 7.16 s, while the execution pipeline took 18.06 s
   and classification reported 18.10 s. C64 alone accounted for 33,916 files
   and about 5.09 s of producer time. Channel send time was only 9.9 ms, so a
   wider channel will not matter.

3. Fresh projection is substantially more expensive than rebuild projection.
   Representative projection time was 19.96 s fresh versus 6.59 s rebuild.
   PMU recorded 14.16 billion cycles in fresh `catalog.persist`; the three
   `catalog.shard.search-index` spans contributed 5.16 billion cycles, including
   4.38 billion for the large C64 shard. The source path is
   `crates/catalog/src/system_shard.rs::write_system_shard`, which calls
   `persisted_search::populate`.

4. Every operation immediately rebuilt Arcade and SNES again after the builder
   persisted. Preview availability reconciliation publishes a new shard in
   `crates/catalog/src/production_sharded_projection.rs::reconcile_production_preview_availability`.
   The post-persist tail was 9.1–10.2 s fresh and 6.3–7.8 s rebuild. Those extra
   generations republished roughly 1.12 MB of Arcade and 2.88 MB of SNES data.

5. The shard pipeline is serial. The primary fresh reconciliation reported
   `shard_workers=1`, `pipeline_peak_in_flight=1`, and zero overlap. Its batch
   took 18.83 s; the slowest C64 shard took 12.82 s. CPU telemetry averaged
   62.5% on CPU0 and 20.2% on CPU1 during fresh work, but the UI thread is on
   CPU1. Rebuild used only about 6.4% of CPU1. There is capacity, but consuming
   it blindly risks the exact scrolling stutter raised before this campaign.

6. exFAT publication is measurable but not the dominant rebuild cost. The main
   fresh generation copied and hashed 31.0 MB in 2.89 s. Storage tracing saw
   4,809 fresh and 5,326 rebuild block requests with zero overruns; the shorter
   rebuild issued more requests, so request count alone does not explain wall
   time.

7. The current search builder repeats work. In
   `crates/catalog/src/persisted_search.rs::populate`, normalized title,
   manufacturer, control, and path values are created for FTS insertion, then
   the original values are normalized again for autocomplete; several labels
   are also regenerated. A `BTreeMap` is updated word-by-word before FTS
   `optimize` and `integrity-check` run. The PMU result makes this a credible
   CPU target, but a subphase marker is needed before choosing between Rust
   normalization, SQLite insertion, optimize, and integrity checking.

## Ranked experiments

### Tier 1 — experiment now

#### 1. Eliminate the immediate preview-availability shard rebuilds

Carry the already-known pack availability into the initial materialization, or
publish preview availability as a small generation-bound overlay rather than
rewriting complete Arcade and SNES shards.

- Conservative impact: 5–8 s from fresh and rebuild durable completion; first
  visible should be unchanged.
- Risks: stale preview flags, overlay/generation mismatch, or a newly downloaded
  pack not becoming visible.
- Small experiment: real Arcade is mandatory; use the same Arcade/SNES/C64
  roots, three control pairs, and a pack-present plus pack-missing case.
- Before/after metrics: `complete_ms`, builder-to-refresh-done tail, generation
  count, republished bytes, and screenshot availability correctness.
- Reject if the affected operation improves by less than 2%, any catalog or
  preview result differs, RSS grows by more than 5%/8 MiB, or any route regresses
  by more than 2%.
- Confidence: high.

#### 2. Reuse classified target output after an exact entry walk

Persist the already-serialized per-target classified output and an exact
fingerprint of the captured directory entries. On rebuild, still enumerate the
real exFAT directory entries for correctness, but replay classified rows for
unchanged targets instead of repeating path classification and preparation.
This is narrower and safer than skipping the namespace walk.

- Conservative impact: 7–11 s on rebuild; 0–2 s on a true fresh build.
- Risks: incomplete fingerprint inputs, rules-version invalidation, cache size,
  and replay ordering differences.
- Small experiment: Arcade/SNES/C64, then mutate one SNES file and one nested
  C64 entry between pairs to prove targeted invalidation.
- Before/after metrics: execution producer/pipeline/classification times,
  reused/invalidated targets, complete time, exact logical fingerprint, and RSS.
- Reject if scan improves by less than 5%, rebuild by less than 2%, any mutation
  is missed, or catalogs differ byte-for-logical-content.
- Confidence: medium-high.

#### 3. Fuse search normalization and instrument FTS subphases

Create one normalized search document per game and reuse it for FTS and
autocomplete. Compute canonical control/player/year/path values once. Replace
the per-token ordered map with a hash accumulator plus one deterministic final
sort. Add separate spans for normalize, FTS insert, autocomplete, optimize, and
integrity-check in the same commit so the benchmark can falsify the mechanism.

- Conservative impact: 2–5 s on a fresh C64-containing build; less than 1 s on
  rebuilds that do not rebuild C64.
- Risks: changed Unicode/token semantics, autocomplete order, and a higher
  transient allocation peak.
- Small experiment: real Arcade/SNES/C64 fresh pair plus persisted-search golden
  queries; no whole-card scan.
- Before/after metrics: `catalog.shard.search-index` cycles and wall time, C64
  shard time, total complete time, search results, autocomplete results, RSS.
- Reject if search-index improves by less than 5%, the fresh operation by less
  than 2%, query results differ, or RSS exceeds the normal gate.
- Confidence: medium-high.

### Tier 2 — valuable, but guard or instrument first

#### 4. Add a UI-aware second shard worker after Arcade readiness

The measured fresh upper bound is about 6.0 s: 18.83 s serial batch minus the
12.82 s slowest shard. Use at most one helper on CPU1, lower priority than the
UI, start only after Arcade is visible, and pause it immediately on input,
catalog-message backlog, or a missed frame budget.

- Conservative impact: 3–5 s fresh and 0.5–1.5 s rebuild.
- Risk: scrolling stutter is high without the adaptive pause; CPU1 owns the UI.
- Qualification: an actual Arcade scrolling workload during every run, under
  ten-second Arcade readiness, zero physical dropped frames, exact catalogs,
  and the standard RSS/regression gates.
- Reject if overlap remains below 2 s, fresh improves by less than 2%, or even
  one qualified physical frame drops.
- Confidence: medium.

#### 5. Hash while writing into same-exFAT staging, then rename

The measured ceiling is about 2.9 s and 31 MB for the main fresh generation,
plus roughly 0.4 s for the immediate Arcade/SNES publications. This is smaller
than earlier whole-card projections and may be offset by slower direct exFAT
writes.

- Conservative impact: 1.5–3.0 s fresh and 0.2–0.6 s rebuild.
- Risks: durability ordering, torn staging after reset, and exFAT writes slowing
  shard construction.
- Experiment: one system at a time, exact post-crash publication checks, then
  the matched three-system benchmark.
- Reject if fresh improves by less than 2%, rebuild regresses by more than 2%,
  or any reset leaves an authoritative partial generation.
- Confidence: medium.

## Rejected or deferred conclusions

- Do not prioritize a durable namespace snapshot from this evidence. Resume
  validation consumed microseconds, not the missing tens of seconds. A snapshot
  may still help a true whole-card build, but it needs separate whole-card data.
- Do not widen the scan channel. Send time was milliseconds against an
  18-second execution pipeline.
- Do not use pprof timings or its catalog as a correctness baseline. One run
  failed fresh/rebuild parity; the confirmation matched internally but produced
  966 Arcade entries instead of the control's 968. Its stacks only establish
  that `library-catalog` dominates `library-walker`.
- Do not take global tracefs ownership. This kernel advertises
  `function_graph`, accepts exact filter symbols, but rejects the tracer in an
  owned instance. The global tracer is outside the safety boundary for this
  live workload.

## Recommended order

Start with preview-availability coalescing because it removes proven duplicate
work with low CPU/UI risk. Then implement classified-output reuse, which attacks
the stable 19-second rebuild scan without trusting exFAT metadata as a complete
namespace snapshot. Instrument and fuse search normalization next. Only after
those should the UI-aware second worker be attempted, because its theoretical
ceiling is smaller and its stutter risk is materially higher.
