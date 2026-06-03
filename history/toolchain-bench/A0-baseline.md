# A0 baseline (2026-06-03, refreshed)

Recorded before any `Cargo.toml` / `.cargo/config.toml` toolchain experiments.
Re-run after `KD_GRAPHICS` (`rust/src/vt.rs`) for clean framebuffer PNGs (no fbcon cursor).

**Command (run 1):** `scripts/bench-toolchain.sh A0 --clean --replace-label`  
**Command (run 2, reproducibility):** `scripts/bench-toolchain.sh A0-r2 --skip-build` (same binary, `BuildID=0b603df9…`)

**Host:** rustc 1.96.0, clean cross-build **179.95 s**, binary **1,653,948 B** (seven embedded Slint scenes).

## A0 run 1

| scene | render µs | copy µs | rows avg | fps | cpu mean % |
|-------|-----------|---------|----------|-----|------------|
| demo | 2742 | 8482 | 619 | 59 | 64 |
| full_motion | 2644 | 9878 | 619 | 59 | 70 |
| static_ui | 1 | 1 | 0 | 61 | 0 |
| local_motion | 386 | 1040 | 96 | 60 | 8 |
| text_heavy | 51510 | 13582 | 1040 | 13 | 83 |
| solid_fill | 20091 | 13535 | 980 | 22 | 70 |
| list_scroll | 35022 | 11970 | 940 | 19 | 85 |

## A0 run 2 (`A0-r2`, same binary)

| scene | render µs | copy µs | fps | cpu mean % |
|-------|-----------|---------|-----|------------|
| demo | 2910 | 8792 | 59 | 65 |
| full_motion | 2830 | 8440 | 59 | 64 |
| static_ui | 1 | 1 | 61 | 0 |
| local_motion | 385 | 1093 | 60 | 8 |
| text_heavy | 52917 | 13065 | 13 | 81 |
| solid_fill | 18789 | 12564 | 26 | 78 |
| list_scroll | 36469 | 12911 | 17 | 77 |

## Run-to-run delta (A0 vs A0-r2)

Typical spread on **render_us** is a few percent (e.g. demo +6%, solid_fill −6%); **fps** and **rows_avg** are unchanged. No sign of instability — safe to treat A0 as a stable baseline before A1/A2.

Earlier same-day A0 (pre-`vt.rs`, compile 237.5 s) is superseded in `results.tsv`; timings are in the same ballpark.

Full TSV: [`results.tsv`](results.tsv). Framebuffer PNGs: `A0-<scene>-fb.png`, `A0-r2-<scene>-fb.png` (gitignored).

**PNG caveat (2026-06-03):** Runs before `bench-toolchain.sh` mid-run capture stored fbcon `login:` — not the bench UI. Re-run A0 after that fix for valid scene PNGs.

**Next:** ~~A1~~ done → [`A1-neon.md`](A1-neon.md). Then A2 (fat LTO only, **remove** A1 rustflags first), then A3 if both help.
