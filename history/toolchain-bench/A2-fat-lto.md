# A2 — fat LTO + codegen-units = 1 (2026-06-03)

**Change only:** `rust/Cargo.toml` `[profile.release]` (A1 rustflags **removed** from `.cargo/config.toml`).

```toml
lto = "fat"
codegen-units = 1
```

**Command:** `scripts/bench-toolchain.sh A2 --clean --replace-label`

**Host:** rustc 1.96.0, clean cross-build **312.60 s**, binary **1,596,604 B** (−57,344 B vs A0 thin LTO).

## A2 on-device (vs latest A0 mid-run run)

| scene | A0 render µs | A2 render µs | Δ | A0 fps | A2 fps |
|-------|--------------|--------------|---|--------|--------|
| demo | 2611 | 2870 | +10% | 59 | 59 |
| full_motion | 2757 | 2646 | −4% | 59 | 59 |
| static_ui | 1 | 1 | — | 60 | 61 |
| local_motion | 362 | 357 | −1% | 60 | 60 |
| text_heavy | 50874 | 56520 | +11% | 13 | 12 |
| solid_fill | 20266 | 19882 | −2% | 21 | 22 |
| list_scroll | 35242 | 36735 | +4% | 17 | 18 |

**Takeaway:** Smaller binary, longer compile. Device fps unchanged on motion scenes; render deltas within noise except `text_heavy` slightly worse. **Not a clear runtime win** vs A0 on these benches.

**PNGs:** `A2-<scene>-fb.png`. **TSV:** `results.tsv` rows `A2`.

**Next:** ~~A3~~ done → [`A3-combined.md`](A3-combined.md).
