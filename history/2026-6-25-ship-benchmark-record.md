# SHIP-20260625-DCD0DBB1 Benchmark Record

Date: 2026-06-25

Scope: generated benchmark evidence for the `SHIP-20260625-DCD0DBB1` device run.
This record preserves the raw TSV rows and calls out which parts are valid
release evidence versus failure evidence.

## Summary

- Warm catalog startup passed across five iterations: first frame was 34-44 ms
  with the summary catalog ready, and full catalog hydration completed around
  439-445 ms.
- Library I/O completed successfully with 9,229 discoveries, 7,897 normal files,
  154 containers, 281 archive entries, and 4,659 unchanged virtual-launch cache
  entries. End-to-end library I/O was recorded as 12 s for this run.
- SQLite publish-only rows stayed around 1.4 s for the 18,026,496 byte catalog.
- Neo Geo screenshot save rows stayed mostly around 1.9-2.0 s, with one 2.4 s
  outlier.
- Neo Geo screenshot download completed successfully: wire download 6.15 s,
  publish/save 2.16 s, verify 1.289 s, total 9.603 s.
- Warm launch-prep completed with zero errors across 120 samples: p50 89 us and
  p95 2,975 us.

## Failure Evidence

The general scene gate rows in `results.tsv` are failure rows, not passing
release evidence. All four scenes under `SHIP-20260625-DCD0DBB1` reported
`ui-rc=2`, `no-fps-lines`, `capture-fail`, and `timing_ok=no`,
`capture_ok=no`, `visual_ok=no`.

The launch-handoff rows also record failure results. The five samples all ended
with `result=error` and `handoff_us` around 750 ms. Keep these rows as evidence
for the run, but do not use them as passing handoff acceptance data.

## Raw Rows

Rows for this label were appended to:

- `history/toolchain-bench/results.tsv`
- `history/toolchain-bench/results-agent.tsv`
- `history/toolchain-bench/results-launch-handoff.tsv`
- `history/toolchain-bench/results-launch-prep.tsv`
- `history/toolchain-bench/results-library-io.tsv`
- `history/toolchain-bench/results-library-save.tsv`
- `history/toolchain-bench/results-screenshot-download.tsv`
- `history/toolchain-bench/results-screenshot-save.tsv`
- `history/toolchain-bench/results-warm-catalog.tsv`

