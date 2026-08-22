# Preview-work deduplication attribution — 2026-08-22

## Decision

Closed without an implementation experiment. The production selected and
prefetch workers performed no overlapping identical work in the fixed real
system-entry, ordinary-scroll, or turbo-scroll routes. An in-flight coordinator
or negative-sidecar cache would therefore add synchronization, invalidation,
and bounded-memory cost without removing measured production work.

## Authority

- Measurement runtime revision:
  `ce0866091d116d291ec227281f37a061dc0151b8`
- Focused host authority fix:
  `71d0b86cfda93334b7990e76e586219eef821a89`
- Passing artifact:
  `build/agent-benchmarks/preview-work-attribution/1787413561`
- Failed pre-measurement artifact:
  `build/agent-benchmarks/preview-work-attribution/1787413485`
- Device boot ID remained
  `bb2f71ac-ed0d-43bd-be6b-e1ddda37507b`.
- Three fresh Arcade system-entry, three five-second ordinary held-scroll, and
  three five-second turbo-scroll controls ran with catalog refresh off.

The first authority attempt stopped before sample one because its system-entry
artifact directory did not exist. The launcher restored normally, the fix made
no runtime change, and the complete rerun passed.

## Results

| Metric | Result | Gate |
| --- | ---: | ---: |
| Preview requests | 503 | context |
| Selected/prefetch colliding requests | 0 (0.00%) | at least 5% |
| Duplicate decodes | 0 | context |
| Duplicate reads | 0 | context |
| Duplicate resizes | 0 | context |
| Repeated missing-sidecar probes | 0 | context |
| Duplicate-work share | 0.00% | at least 2% |
| Aggregate read time | 122.522ms | attribution |
| Aggregate decode/parse time | 749.681ms | attribution |
| Aggregate worker CPU | 749.399ms | attribution |
| System-entry selected-preview p50 | 11.513ms | at most 85ms p95 |
| System-entry selected-preview p95/max | 12.428ms | at most 85ms p95 |

Route request counts were 12/12/12 for fresh system entry, 52/50/50 for
ordinary scroll, and 105/105/105 for turbo scroll. Every route independently
reported zero collisions and zero duplicate work. Selected worker queue age
was 1.577–2.636ms; the longer prefetch queue age remained background-only and
did not delay the selected result.

All three system-entry results preserved the terminal selected preview and
generation. All six scrolling profiles completed with no failure and zero
dropped-frame records. Catalog refresh was not forced.

## Interpretation

The shared decoded cache is already sufficient for the measured request
patterns. Selected requests either target a different key from active prefetch
work or arrive after the prefetched pixels are cache-visible. Sidecar indexes
were present, so the negative-probe portion also had no production opportunity
in this corpus. Reopen only when a new production corpus records at least 2%
duplicate work or a route records at least 5% exact-key collisions.
