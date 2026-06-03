# A3 — NEON + fat LTO combined (2026-06-03)

**Changes:** A1 `rust/.cargo/config.toml` rustflags **and** A2 `Cargo.toml` `lto = "fat"`, `codegen-units = 1`.

**Command:** `scripts/bench-toolchain.sh A3 --clean --replace-label`

**Host:** rustc 1.96.0, clean cross-build **287.92 s**, binary **1,612,996 B** (−40,952 B vs A0; +16,392 B vs A2 alone).

## A3 vs A0 (reference baseline, mid-run PNG run)

| scene | A0 render µs | A3 render µs | Δ | A0 fps | A3 fps |
|-------|--------------|--------------|---|--------|--------|
| demo | 2611 | 2581 | −1% | 59 | 59 |
| full_motion | 2757 | 2547 | −8% | 59 | 59 |
| static_ui | 1 | 1 | — | 60 | 61 |
| local_motion | 362 | 367 | +1% | 60 | 60 |
| text_heavy | 50874 | 55728 | +10% | 13 | 12 |
| solid_fill | 20266 | 19647 | −3% | 21 | 24 |
| list_scroll | 35242 | 34862 | −1% | 17 | 18 |

## Matrix summary

| Label | compile s | bytes | demo render µs | full_motion fps | list_scroll fps |
|-------|-----------|-------|----------------|-----------------|-----------------|
| A0 | 180 | 1,653,948 | 2611 | 59 | 17 |
| A1 | 271 | 1,666,244 | 2605 | 59 | 16 |
| A2 | 313 | 1,596,604 | 2870 | 59 | 18 |
| A3 | 288 | 1,612,996 | 2581 | 59 | 18 |

## Recommendation

- **Ship A2 or A3 release profile** if you want a **smaller binary** (~40–57 KB less than A0) and accept **~5+ min clean cross-builds** (fat LTO).
- **NEON alone (A1)** did not help; combined **A3** is in the noise band except modest gains on `full_motion` / `solid_fill` / `list_scroll` in this single run.
- **60 fps motion scenes** (`demo`, `full_motion`) are unchanged across all labels — the bottleneck is copy/vsync, not a few µs of render on those benches.
- **Heavy scenes** (`text_heavy`, `list_scroll`) remain CPU-bound; toolchain tweaks won’t fix without UI/renderer changes.

Current repo defaults match **A3** (fat LTO + NEON rustflags). Revert to A0 in `Cargo.toml` / `.cargo/config.toml` if you prefer faster incremental builds during dev.

**PNGs:** `A3-<scene>-fb.png`. **TSV:** rows `A3`.
