# RGB565 Preview Fade NEON Evaluation

Date: 2026-07-10

## Result

Keep the production RGB565 preview-fade row blend scalar. Remove the unreachable
Rust NEON branches and their dispatch wrappers.

The optimized ARM build passes `-C target-feature=+neon`, but stable Rust does
not expose `target_feature="neon"` in its ARM cfg set. The preview-fade Rust
NEON functions were therefore not compiled on MiSTer and the scalar row blend
was the actual production path.

## Device evidence

A 20-second real `turbo-hold` trace captured 859 active 200ms fade frames:

| Metric | Result |
| --- | ---: |
| average fade CPU | 1.326ms |
| p95 fade CPU | 1.546ms |
| p99 fade CPU | 1.619ms |
| average fade pixels | 92,351 |

A bench-tools-only C NEON probe used the exact RGB565 blend arithmetic on a
320×320 double-image fade (102,400 pixels), at alpha buckets used by the real
trace. Its output matched scalar byte-for-byte but was slower:

| Alpha bucket | Scalar p95 | C NEON p95 | NEON speed |
| --- | ---: | ---: | ---: |
| 5 | 1.802ms | 2.444ms | 0.74x |
| 16 | 1.737ms | 2.445ms | 0.71x |
| 27 | 1.735ms | 2.446ms | 0.71x |

The temporary C probe and bench command were removed after measurement. No
production C helper was added: the existing scalar blend is faster on this
Cortex-A9 workload.
