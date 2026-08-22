# Generation-stable search attribution — 2026-08-22

## Decision

Closed without a retained-worker experiment. Per-query SQLite open and
statement preparation are real, but their combined p95 is only 3.031ms, or
12.14% of the 24.962ms total p95. Even deleting both phases cannot meet the 20%
gate. The production Arcade UI route also used its resident search projection
and created no persisted-search worker, so a long-lived worker would not improve
the qualified core interaction.

## Authority

- Measurement runtime revision:
  `43912c1b510484ae176d85ff0136d8aaf03c22a9`
- Passing artifact: `build/agent-benchmarks/search/1787414670`
- Earlier direct-only artifact:
  `build/agent-benchmarks/search/1787414216`
- Three complete suites ran `pac`, `street`, `capcom`, and `2 player` against
  one validated Arcade generation, with 20 measured iterations per query.
- Every iteration preserved exact system IDs, ordinals, rank bits, and
  autocomplete hashes.
- Catalog refresh remained off throughout the launcher qualification.

## Results

| Metric | Current baseline | Historical context |
| --- | ---: | ---: |
| Total p50 | 6.396ms | 6.339ms |
| Total p95 | 24.962ms | 25.996ms |
| Total max | 25.286ms | not recorded |
| SQLite open p95 | 0.623ms | not recorded |
| Statement prepare p95 | 2.408ms | not recorded |
| SQLite execute p95 | 21.115ms | not recorded |
| Rust finalize p95 | 0.979ms | not recorded |
| SQLite opens | 264 | not recorded |
| Statement prepares | 528 | not recorded |
| Minor faults | +182 | not recorded |
| RSS/HWM | 6,496KiB → 9,192KiB | not recorded |

`2 player` remained the dominant query. Its three p50 values were 24.949ms,
24.870ms, and 21.988ms; its p95 values were 25.286ms, 25.160ms, and 24.288ms.
All four query hashes were identical across the three complete suites.

The production launcher qualifier entered cached Arcade through the shared
system-entry handoff, opened Search through bounded automation, and returned
922 exact results for `A` in 262ms. It observed zero persisted-search worker
creations because the resident catalog projection answered the query.

## Attribution

Open plus preparation accounts for 12.14% of total p95 and therefore reaches
only 60.7% of the required 20% improvement gate. SQLite execution accounts for
84.6% of total p95 and is dominated by the broad `2 player` result set. The
roadmap permits a recovery only after reaching 80% of the gate, so no retained
connection/statement experiment is justified by this corpus. Reopen only if a
nonresident production route creates repeated workers or open/prepare churn
grows to at least 16% of p95.
