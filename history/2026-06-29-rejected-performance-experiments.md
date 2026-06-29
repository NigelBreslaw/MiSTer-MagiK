# 2026-06-29 Rejected Performance Experiments

This note records experiments that were measured and intentionally not kept in
production code. Runtime code should contain only the production path unless an
experiment is actively being tested.

## Arcade Dense Present

Commit `69aa0321` added an env-gated
`MISTER_ARCADE_LIST_PRESENT=segmented|dense` experiment. Dense mode copied the
full arcade list rectangle and repainted the selection frame in the scratch
surface before presenting.

Benchmarks:

```text
scripts/profile-arcade-scroll.sh PERF-P08-BEFORE-20260629 --secs 30 --scenario turbo-hold --skip-build
scripts/profile-arcade-scroll.sh PERF-P08-DENSE-20260629 --secs 30 --scenario turbo-hold --skip-build --list-present dense
```

Result:

| label | arcade_list_present_us p95 | fb_present_us p95 |
| --- | ---: | ---: |
| `PERF-P08-BEFORE-20260629` | 556us | 1017us |
| `PERF-P08-DENSE-20260629` | 1515us | 2113us |

Dense mode preserved the selection frame visually, but it more than doubled the
owned present timings. The production segmented present path remains the only
runtime path.

## P10 Cold Catalog Attempts

Two cold catalog construction attempts were tested and rejected:

- Shared `LibraryScanArtifact` materialization helper:
  `catalog_us` regressed from `4.125s` to `4.203s`.
- Filtered MAME software metadata load for the RAM catalog:
  `library_ready` improved slightly, but owned `catalog_us` regressed from
  `4.125s` to `4.641s`.

Both patches were discarded before commit. The saved local rejected patches were
`/private/tmp/p10-reject.patch` and `/private/tmp/p10-filter-reject.patch` on
the development machine at the time of measurement.
