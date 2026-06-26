# Index Pread Payload Buffer Experiment Dropped

Date: 2026-06-26

## Experiment

Tested item 2 from the screenshot decode/fewer-copy plan: reuse a compressed
payload buffer inside `PreviewArchiveScratch` for `.mmlz4b.idx`/`pread` preview
loads, avoiding a fresh compressed-payload allocation per cold selected preview
request.

The branch extended the scratch state with a reusable payload buffer and used it
only in the sidecar index `pread` path. The code was dropped after the benchmark
gate failed.

## Commands

Baseline:

```bash
scripts/deploy-rust.sh --ui-scope launcher
for i in 01 02 03 04 05 06 07 08 09 10; do
  scripts/profile-first-preview.sh PREADBUF-BEFORE-$i --secs 8 --skip-build
done
```

After experiment:

```bash
scripts/deploy-rust.sh --ui-scope launcher
for i in 01 02 03 04 05 06 07 08 09 10; do
  scripts/profile-first-preview.sh PREADBUF-AFTER-$i --secs 8 --skip-build
done
```

## Results

Selected first-preview rows:

```text
PREADBUF-BEFORE decoded_total_p95_us=22402 apply_age_p95_us=48585 all_index_pread=true
PREADBUF-AFTER  decoded_total_p95_us=22798 apply_age_p95_us=48814 all_index_pread=true
```

The gate required all selected samples to use `index_pread`, at least 8%
improvement in p95 `decoded_total_us`, and no p95 `apply_age_us` regression.
The load-source condition passed, but decoded p95 regressed by 1.8% and apply
age p95 regressed by 0.5%.

## Decision

Drop the reusable payload-buffer implementation. Allocation removal was too
small to overcome run-to-run noise and did not improve the selected cold
first-preview latency metric.

