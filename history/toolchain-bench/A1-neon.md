# A1 — Cortex-A9 + NEON rustflags (2026-06-03)

**Change only:** `rust/.cargo/config.toml`

```toml
[target.armv7-unknown-linux-gnueabihf]
rustflags = ["-C", "target-cpu=cortex-a9", "-C", "target-feature=+neon,+vfp3"]
```

**Prerequisite:** `scripts/audit-mister.sh` → `A1 prerequisite: OK` (CPU part `0xc09`, NEON, VFPv3).

**Command:** `scripts/bench-toolchain.sh A1 --clean --replace-label`

**Host:** rustc 1.96.0, clean cross-build **271.38 s** (+51 s vs A0), binary **1,666,244 B** (+12,296 B vs A0).

## A1 on-device (vs latest A0 mid-run PNG run)

| scene | A0 render µs | A1 render µs | Δ | A0 fps | A1 fps |
|-------|--------------|--------------|---|--------|--------|
| demo | 2611 | 2605 | ~0% | 59 | 59 |
| full_motion | 2757 | 2647 | −4% | 59 | 59 |
| static_ui | 1 | 1 | — | 60 | 61 |
| local_motion | 362 | 386 | +7% | 60 | 60 |
| text_heavy | 50874 | 54564 | +7% | 13 | 12 |
| solid_fill | 20266 | 19474 | −4% | 21 | 23 |
| list_scroll | 35242 | 35585 | +1% | 17 | 16 |

Run-to-run noise is a few percent; **no clear win** on hot paths (`demo`/`full_motion` unchanged fps). `text_heavy` slightly slower on render; `solid_fill` slightly faster. Worth keeping for A3 combined test, not a standalone slam-dunk.

**PNGs:** `A1-<scene>-fb.png` (mid-run capture). **TSV:** `results.tsv` rows labeled `A1`.

**Next:** ~~A2~~ done → [`A2-fat-lto.md`](A2-fat-lto.md). Then A3 (A2 + A1 rustflags).
