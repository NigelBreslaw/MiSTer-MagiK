# A0 baseline (2026-06-03)

Recorded before any `Cargo.toml` / `.cargo/config.toml` toolchain experiments.

**Command:** `scripts/bench-toolchain.sh A0 --clean --replace-label`  
**Host:** rustc 1.96.0, clean cross-build **237.5 s**, binary **1,653,948 B** (seven embedded Slint scenes).

| scene | render µs | copy µs | rows avg | fps | cpu mean % |
|-------|-----------|---------|----------|-----|------------|
| demo | 2781 | 7896 | 619 | 60 | 59 |
| full_motion | 2815 | 8500 | 619 | 59 | 64 |
| static_ui | 1 | 1 | 0 | 61 | 0 |
| local_motion | 342 | 985 | 96 | 60 | 8 |
| text_heavy | 56910 | 14399 | 1040 | 12 | 81 |
| solid_fill | 19361 | 13324 | 980 | 26 | 89 |
| list_scroll | 35359 | 12533 | 940 | 17 | 78 |

Full TSV: [`results.tsv`](results.tsv). Framebuffer PNGs: `A0-<scene>-fb.png` (gitignored).

**Next:** A1 (+neon only), then A2 (fat LTO only), then A3 if both help — one change per label.
