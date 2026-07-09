# Raw565 Fade Optimization Experiments

Date: 2026-07-08

## Context

Raw565 preview fade composition was measured as a main UI-thread hot path. The goal was to test small, isolated RGB565 fade optimizations against the existing 32-bucket visual contract, using MiSTer thread CPU timing rather than host timing.

The shared timing system was kept:

- `preview_fade_wall_us`
- `preview_fade_cpu_us`
- `preview_fade_pixels`
- `preview_fade_rows`
- `preview_fade_path`
- `preview_fade_alpha_bucket`
- `fade_cpu_tsv`
- `fade_bucket_tsv`

The experiment switches and kernels were dropped after measurement.

## Experiments

Baseline was the committed default RGB565 fade path. Four temporary experiment commits were tested:

- `bucket-shift`: no-multiply kernels for alpha buckets 8, 16, and 24.
- `rows-black-neon`: route previous-only/current-only row segments through the black-row helper.
- `neon8`: alternate 8-pixel ARM NEON same-geometry row kernel.
- `scaled-affine`: precomputed/incremental source coordinate mapping for `scaled_sample`.

Each mode ran three normal 30 second `turbo-hold` repetitions and one CPU-profile repetition on MiSTer. CPU SVGs were generated under `build/preview-scroll-profiles/` during the experiment and are build artifacts, not history source.

## Normal Run Results

Active fade frames only, aggregated across three repetitions:

| mode | frames | avg cpu | p95 cpu | p99 cpu | cpu ns/pixel | paths |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| baseline | 3922 | 1382.0us | 1625us | 1693us | 14.931 | rows, same_geometry, single_black, cut |
| bucket-shift | 3924 | 1514.9us | 2054us | 2194us | 16.368 | rows, same_geometry, single_black, cut |
| rows-black-neon | 3923 | 1401.2us | 1646us | 1706us | 15.141 | rows, same_geometry, single_black, cut |
| neon8 | 3923 | 1380.1us | 1626us | 1690us | 14.912 | rows, same_geometry, single_black, cut |
| scaled-affine | 3927 | 1376.6us | 1624us | 1693us | 14.876 | rows, same_geometry, single_black, cut |

## CPU-Profile Results

Active fade frames only, one CPU-profile repetition per mode:

| mode | frames | avg cpu | p95 cpu | p99 cpu | cpu ns/pixel |
| --- | ---: | ---: | ---: | ---: | ---: |
| baseline | 1308 | 1406.0us | 1676us | 1769us | 15.192 |
| bucket-shift | 1308 | 1587.6us | 2269us | 2481us | 17.154 |
| rows-black-neon | 1308 | 1436.0us | 1719us | 1827us | 15.519 |
| neon8 | 1308 | 1403.0us | 1680us | 1767us | 15.164 |
| scaled-affine | 1312 | 1407.6us | 1703us | 1811us | 15.221 |

All CPU-profile runs generated CPU SVG artifacts. The benchmark command returned exit 6 for each profile run because of the existing preview visibility gate miss: 4 empty-cache frames out of 1798. The trace, fade timing, and CPU SVG artifacts were still produced.

## Decision

No experiment met the acceptance bar. There was no 20% improvement in `preview_fade_cpu_us` p95/p99 or `cpu_ns_per_pixel`.

- Reject `bucket-shift`: it regressed badly, especially bucket 16.
- Reject `rows-black-neon`: consistently slower than baseline.
- Reject `neon8`: effectively tied; tiny average movement was noise-level and p95/p99 did not improve.
- Reject `scaled-affine`: not exercised on the intended `scaled_sample` path by `turbo-hold`; profile data did not show an overall win.

Keep the timing system. Drop the experiment code.
