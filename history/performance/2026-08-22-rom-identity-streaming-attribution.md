# ROM identity streaming attribution — 2026-08-22

## Scope

- Baseline implementation: whole-file allocation, transformed candidate
  allocation, scalar bit-oriented CRC32.
- Candidate implementation: one 256 KiB read buffer, fused incremental table
  CRC32 candidates, bounded transform carry, cooperative checkpoints.
- Production default remained the whole-file implementation.
- Catalog refresh remained off; the benchmark did not construct a catalog.
- Device: exact Dev runtime at MagiK `ad8966e9f`, Main `639d3694e`.

## Evidence

Initial whole-file authority:

- `build/agent-benchmarks/rom-identity-hashing/1787405303`
- `build/agent-benchmarks/rom-identity-hashing/1787405372`
- `build/agent-benchmarks/rom-identity-hashing/1787405398`

Balanced comparison captures (baseline A, candidate A, candidate B, baseline B):

- `build/agent-benchmarks/rom-identity-hashing/1787406145`
- `build/agent-benchmarks/rom-identity-hashing/1787406220`
- `build/agent-benchmarks/rom-identity-hashing/1787406267`

All six baseline and six candidate arms produced result SHA-256
`3814bb7244267dd8b5b2f83826e10f089c0e9479ce93c84344ba89b00321c4f9`
and software-cache SHA-256
`d3270ed70837291e7b66eef46926f8064837b074c16bc4b0f2ff9b4a87dd8213`.

## Six-arm medians

| Measurement | Whole-file | Streaming | Delta |
|---|---:|---:|---:|
| Production-default Lynx identity | 9.193 ms | 5.651 ms | -38.5% |
| Lynx read | 1.028 ms | 0.776 ms | -24.6% |
| Lynx CRC / fused processing | 7.177 ms | 4.467 ms | -37.8% |
| Sum of nine selected identity cases | 6.676 s | 2.888 s | -56.7% |
| Large N64 identity | 4.505 s | 1.913 s | -57.5% |
| Process HWM | 188,038 KiB | 24,020 KiB | -87.2% |
| Large N64 transformed candidate allocation | 134,217,728 B | 0 B | -100% |

The maximum candidate checkpoint was 63 microseconds. Every selected size class
improved: Lynx small 38.5%, Mega Drive small 43.0%, Mega Drive medium 45.2%,
N64 small 29.2%, N64 medium 57.4%, N64 large 57.5%, NES small 47.9%, SNES small
54.5%, and SNES medium 52.0%.

## Gate attribution

The live scanner cache contains 321 production-default Lynx hash entries. At
the median per-entry saving of 3.542 ms, the conservative projected scan saving
is 1.137 s. Against the historical 70.190 s scan this is 1.62%, below the 2%
affected-operation gate even though the identity-phase and memory gates pass.

Candidate processing remains dominant at 4.467 ms of the 5.651 ms Lynx total.
The planned bounded recovery is therefore justified: try one slicing-by-eight
incremental CRC implementation. Promotion remains blocked until that recovery
is measured; production behavior is unchanged at this revision.
