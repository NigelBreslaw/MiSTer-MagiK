# C64 artifact writer experiments

Date: 2026-08-26

Branch: `nigel/arcade-catalog-prototype`

## Retained result

All samples rebuilt only the new C64 artifact set after a verified supervised
reboot. Every sample reproduced all 15,089 rows and passed published-shard and
persisted-search validation. The existing old-builder result was not rerun.

| Strategy | C64 artifact time | Change |
| --- | ---: | ---: |
| Build directly on exFAT | 21.656 s | baseline |
| Build in tmpfs, copy once to exFAT | 11.118 s | 48.7% faster |

The retained strategy keeps SQLite, the full FTS5 index, FTS optimization, the
FTS integrity check, NavPack, and navigation output. Only the construction
location and publication pattern change. SQLite performs its random-write
work in tmpfs; publication then writes the completed 26,400 KB artifact set to
exFAT sequentially and synchronizes it.

This saves 10.537 seconds and makes the focused C64 artifact stage 1.95x
faster. A second cold run of the retained tmpfs strategy took 11.245 seconds,
confirming the result.

## Rejected experiments

- Increasing SQLite's cache to 16 MB and using memory-backed temporary tables
  did not materially improve the 11.1-second result.
- Disabling FTS optimization reduced the artifact stage from 11.245 to 10.128
  seconds and reduced SQLite from 19,960 KB to 17,980 KB. Exact search results
  and ranking were preserved, but the cold search probe became 29.8% slower.
  That trade is rejected because interactive search responsiveness is more
  important than another one-second first-build saving.
- Deferred SQLite durability did not improve construction in tmpfs; exFAT copy
  variance dominated that sample.

After tmpfs staging, FTS construction is about 3.1 seconds. It is no longer the
largest avoidable cost, and removing or weakening it is not justified by these
measurements.

## Evidence

- Staging and SQLite-memory matrix:
  `build/agent-benchmarks/fast-five-c64-experiments/ebf914aad/report.json`
- Two-reboot optimized versus unoptimized FTS comparison:
  `build/agent-benchmarks/fast-five-c64-experiments/454c2fce6/report.json`

The production registry was unchanged and the isolated device artifacts were
removed after each matrix.
