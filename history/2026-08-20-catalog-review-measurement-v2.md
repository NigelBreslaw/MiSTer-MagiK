# Independent catalog measurement review v2

Frozen evidence commit: `9f7833056`

Qualified runtime: `99f6b36399563ed8b5aef883f93a7ca8921e0402`

Role: measurement skeptic/statistician. This review was performed read-only and
without access to the other two reviews.

## Evidence integrity

All 14 paths in `2026-08-20-catalog-optimization-qualification-v2.json`
exist and match their declared SHA-256 values.

The bounded arithmetic is correct:

- Fresh completion median: 91.260 s; range 90.050-92.235 s.
- Changed-rebuild median: 30.248 s; range 29.632-30.398 s.
- Fresh first-visible median: 9.227 s; range 9.198-9.793 s.
- Changed-rebuild first-visible median: 2.295 s; range 2.148-2.574 s.
- Every sample has the expected one-game SNES delta and stable within-leg
  fingerprints.

The whole-card values are also transcribed correctly: 219.913 s first clean,
165.110 s warm clean, and 66.071 s forced unchanged rebuild for 69 systems and
39,791 games.

Relative to `dd6e513`, those observations are 4.85%, 33.24%, and 43.33%
faster. Relative to the pre-campaign observations, first clean is 19.18%
faster and rebuild is 61.01% faster. The single-global-autocomplete result is
also correct: the C64 row-loop median improved 3.45% and total search improved
2.37%, below its 5% phase gate.

## Strong attribution

The post-scan unchanged exit has the strongest causal evidence. The prior
builder took 98.883 s. The qualified builder spends 47.152 s scanning and
0.543 s deciding, for 47.694 s total. That 51.189 s reduction agrees with the
50.521 s wall reduction to within 0.668 s. The source anchors are
`builder_service.rs::SystemBuilderBackend::decide_after_scan` and the early
return in `run_with_backend_policy`.

The remaining unchanged-rebuild ceiling is the 46.921 s scan, 71.0% of the
66.071 s harness wall. Further tuning of the 0.543 s decision cannot matter.

The retained MRA prefix read/four-reader prefetch, compact checkpoint frames,
work-mode epochs, persisted-search pipeline, post-scan exit, and explicit
reverts are all present in the qualified ancestry. Only the unchanged exit has
clean causal evidence in this v2 sequence. Older compact-checkpoint evidence
provides its separate A/B; the other mechanisms remain combined observations.

## Measurement dissent

1. `catalog_logical_fingerprint` hashes only system rows and total count. It
   does not cover individual game identity, titles, paths, launch plans,
   ordering, search results, or published artifact bytes. A different catalog
   can therefore pass the current fingerprint.
2. Whole-card evidence is one fixed-order fresh/warm/rebuild sequence with
   unchanged page cache. It is correctness evidence and a directional timing
   observation, not a statistical distribution or isolated attribution.
3. Status polling, telemetry, and catalog inspection are inside the harness
   interval. Completion has subsecond-to-second observation latency and some
   observer cost.
4. “First visible” is the first `catalog_ready` status observation, not a
   visual timestamp. The log separately proves Home with Arcade selected.
5. Production-registry protection hashes manifest slots, not every referenced
   artifact.
6. Reported HWM is the builder process at selected stages, not whole-device
   memory pressure or page cache.
7. The zero-telemetry attempt followed by one successful retry introduces mild
   survivorship bias.
8. Whole-card UI qualification is non-applicable. Bounded interaction passing
   is not authoritative zero-repeat animation evidence.

No causal Arcade-readiness gain can be assigned to MRA concurrency, work-mode
bursting, or search pipelining from the frozen A/B evidence alone.

## Top ideas: instrument first

### 1. Exact per-target unchanged verification

Persist a content-complete target fingerprint and reclassify only targets that
change. Do not decode old output merely to determine unchanged state.

- Measured ceiling: 46.921 s unchanged scan.
- Conservative impact: unchanged rebuild 20-35 s; changed rebuild 5-15 s;
  fresh and Arcade readiness approximately zero.
- Small experiment: one warmup plus three unchanged and three single-change
  pairs using real Arcade, C64, SNES, and an archive-heavy recursive root.
- Measure complete wall, scan, target hit/miss/fallback, canonical per-game
  digest, artifact SHA set, and changed-target set.
- Falsify below 10% whole-operation gain, on any missed mutation, or on any
  exact digest/artifact mismatch.
- Risks: exFAT timestamp granularity, renames, content-only changes, stale MRA
  launch metadata, and persistent-index growth.
- Confidence: medium.

### 2. Overlap finalized-system projection with remaining scan

Use proven system-finality events to start deterministic per-system
shard/search construction while slow roots continue scanning.

- Measured ceiling: 64.997 s fresh scan plus 43.251 s projection.
- Conservative impact: fresh 10-20 s; changed rebuild 3-8 s; unchanged rebuild
  near zero; Arcade readiness near zero if Arcade retains priority.
- Small experiment: real Arcade plus C64, SNES, PC8801, GBA, MegaDrive, NES,
  and GBC; one warmup and three fresh samples, then whole-card confirmation.
- Falsify below 8% fresh gain, below 10 s overlap, above 15% HWM increase, or
  on any exact digest, ownership, or physical-frame failure.
- Risks: finality errors, nondeterministic order, RAM overlap, SD contention,
  and UI starvation.
- Confidence: medium-low until exact artifact digests exist.

### 3. Immutable per-system search cache

Key exact search artifacts by a complete system-content and search-contract
hash and reuse them on changed rebuilds.

- Conservative impact: changed rebuild 5-12 s when most systems are unchanged;
  fresh, unchanged rebuild, and Arcade readiness approximately zero.
- Small experiment: real Arcade/C64/SNES with three fresh and one-SNES-change
  pairs, followed by one whole-card changed-root confirmation.
- Measure changed-rebuild wall, per-system search phases, hit bytes/count,
  canonical query-result digest, artifact SHA, and HWM.
- Falsify below 10% changed-rebuild gain, if validation consumes more than 25%
  of saved construction time, or on any query/artifact mismatch.
- Risks: schema/tokenizer invalidation, cache growth, corruption, and atomic
  publication.
- Confidence: medium-low.

Before any optimization, strengthen the benchmark fingerprint to cover
canonical games, launch contracts, ordering, search results, and every
published artifact SHA.
