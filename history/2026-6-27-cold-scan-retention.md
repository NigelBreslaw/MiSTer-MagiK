# Cold Scan Retention

This note records the first pass over the cold-scan retention policy: keep
changes that improve perceived startup, but require non-UX scan optimizations to
remove at least 8s from cold scan time before they earn extra code complexity.

## Policy

- Judge scanner optimizations against `library_scan_complete`,
  `scan_stage_walk`, `scan_stage_file_discovery`, and
  `scan_stage_classify_total`.
- First-scan acceptance now requires `library_ready <= 41000ms` for RAM catalog
  usability and `library_db_saved <= 55000ms` for durable SQLite save
  completion.
- Do not count `library_ready`, `library_db_saved`, SQLite import/publish, or
  saved-catalog hydration toward the 8s scanner bar.
- Track `counter_plateau` as the first-scan "felt stuck" metric.
- Track `catalog_worker_ram_catalog` separately from scanner and SQLite costs.

## 2026-06-27 Measurements

All runs used `scripts/profile-first-scan.sh`, which deletes the production
catalog database and summary projection, syncs, reboots the MiSTer, and waits
for both `library_ready` and `library_db_saved`. These runs were collected from
the dirty retention worktree before the script learned to append `-dirty` to
the commit field for uncommitted benchmark builds.

| label | prefetch workers | library_scan_complete | scan_stage_walk | scan_stage_file_discovery | scan_stage_classify_total | library_ready | counter_plateau |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| RETAIN-HEAD-20260627 | 2 default | 35463ms | 28157ms | 30947ms | 34411ms | 40679ms | 20983ms |
| RETAIN-PREFETCH3-20260627 | 3 | 35214ms | 26692ms | 30846ms | 34224ms | 40502ms | 23467ms |
| RETAIN-PREFETCH4-20260627 | 4 | 35215ms | 24405ms | 30897ms | 34244ms | 40506ms | 24750ms |
| RETAIN-NOPROGRESSUI-20260627 | 2 default, progress UI off | 34462ms | 27424ms | 30328ms | 33560ms | 39448ms | n/a |
| RETAIN-NOWORKERS-20260627 | sequential target streaming | 34680ms | 33025ms | 29844ms | 33675ms | 39906ms | 11051ms |

## Decisions

- Keep the first-scan measurement additions. They expose RAM catalog projection
  and the visible counter plateau directly in the normal TSV.
- Keep the reboot recovery and DB scalar parsing hardening as benchmark
  correctness support: they ensure first-scan runs are true cold reboot runs and
  acceptance scripts read the deployed catalog count consistently. They are not
  counted as scan-time optimizations.
- Keep the honest bootstrap counter behavior. The UI should not inflate a real
  bootstrap count to 1000 just to keep the number moving.
- Do not change the default target prefetch worker count from 2 to 3 or 4. More
  workers reduce `scan_stage_walk`, but the end-to-end scan completion win is
  only about 0.25s and the counter plateau gets worse, so this misses the 8s
  retention bar. The target prefetch worker pool was removed after this
  measurement so target walking streams progress in order again.
- Sequential target streaming keeps scan completion in the same range while
  cutting the visible counter plateau roughly in half on this run. This is a UX
  win, not an 8s scan-time win.
- Catalog progress UI updates appear to cost about 1s of cold scan completion
  on this run. That is worth knowing, but it is not the main 8s scanner problem.
  The no-progress run emitted no counter milestones because catalog progress UI
  messages were suppressed. The no-progress UI benchmark mode was removed after
  this measurement.

## Open Candidate

`scan_stage_file_discovery` remains the largest scanner cost center. A future
candidate should test bounded parallel classification or metadata fast paths,
but must preserve deterministic catalog order and launch correctness.
