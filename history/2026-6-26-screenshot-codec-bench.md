# Screenshot Codec Benchmarks

Date: 2026-06-26

## Context

Screenshot codec experiments must use the installed MiSTer MagiK screenshot pack
corpus, not the full private source screenshot pool. The arcade workload tested
here used the device-derived arcade pack: 832 entries, 132,773,120 raw bytes.
Saturn and Neo Geo were recoded only for pack-size comparison rows.

The decode benchmark loaded the full arcade pack into memory before timing and
then decoded every entry in random order for three iterations. Launcher turbo
tests were gated by `warm_gate_tsv loaded=1 valid=1` so the 60 second timing
window did not include full-pack warmup.

## Results

Current LZ4 fast baseline:

- Arcade pack: 37,004,558 bytes.
- Saturn pack: 10,979,745 bytes.
- Neo Geo pack: 9,019,802 bytes.
- Decode p50/p95/p99/max: 1076 / 1496 / 1645 / 3849 us.

Best LZ4-HC candidate:

- `mmlz4b-lz4-hc-9`.
- Arcade pack: 28,475,693 bytes.
- Saturn pack: 9,612,492 bytes.
- Neo Geo pack: 6,583,775 bytes.
- Decode p50/p95/p99/max: 853 / 1477 / 1765 / 2122 us.
- Turbo UI run had `index_pread=0` and fewer slow-frame outliers than baseline.

ZXC was tested as a benchmark-only experiment and rejected:

- `zxc-1` arcade pack was larger than baseline at 44,563,974 bytes and decoded
  at p50/p95/p99/max 1126 / 1906 / 2299 / 7657 us.
- `zxc-5` arcade pack was 31,480,908 bytes but decoded at
  p50/p95/p99/max 1459 / 2564 / 3099 / 8379 us.
- `zxc-6` arcade pack was 29,529,745 bytes but decoded at
  p50/p95/p99/max 1570 / 3041 / 4219 / 5479 us.
- Conclusion: ZXC density did not offset Cortex-A9 decode cost.

C `liblz4` decode was tested and rejected:

- Best-looking measured C variant was the standalone GCC `-O3` build with
  `LZ4_FAST_DEC_LOOP=1` and `LZ4_FORCE_MEMORY_ACCESS=2`.
- Corrected thread-CPU timing on the same HC-9 arcade pack gave decode
  p50/p95/p99/max 778 / 1494 / 1824 / 2264 us.
- The Rust `lz4_flex` HC-9 path on the same corrected run gave thread-CPU
  p50/p95/p99/max 749 / 1272 / 1509 / 1754 us.
- Conclusion: C `liblz4` compile matrices and PGO are not worth pursuing for
  this Cortex-A9 path. Stick with Rust `lz4_flex` and optimize the work around
  decode instead.

## Decision

Promote the positive result to production: screenshot packs use
`mmlz4b-v2-lz4-hc-9-pixels`, which keeps LZ4-HC-9 and compresses only RGB565
pixel bytes rather than the 20-byte `MM56501` wrapper. Do not keep ZXC
reader/writer or C `liblz4` decoder code in the repo. Preserve those failed
paths as history only.

## Next Bets

1. Build a per-entry Pareto packer that raw-stores or changes level only for
   assets that buy p95/p99/max decode improvements.
