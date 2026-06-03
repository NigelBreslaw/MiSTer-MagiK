# Slint `master` (git) — project default (2026-06-03)

Benchmark vs crates.io **1.16.1** (A3). Same host toolchain: `release-device`, cross **0.2.5**, NEON rustflags.

## Cargo.toml

```toml
[patch.crates-io]
slint = { git = "https://github.com/slint-ui/slint", branch = "master" }
slint-build = { git = "https://github.com/slint-ui/slint", branch = "master" }
```

Lockfile resolves to **slint 1.17.0** at git **`9f5e4a49`** (2026-06-03).

## Host build

| | A3 (crates.io 1.16.1) | slint-master |
|--|------------------------|--------------|
| Clean cross-build (`release-device`) | 288 s | **~378 s** (6m 18s) |
| Binary | 1,612,996 B | **1,580,228 B** (−33 KB) |

## On-device vs A3

| scene | A3 render µs | slint-master render µs | A3 fps | slint-master fps |
|-------|--------------|------------------------|--------|------------------|
| demo | 2581 | 2619 | 59 | 60 |
| full_motion | 2547 | 2413 | 59 | 59 |
| static_ui | 1 | 1 | 61 | 61 |
| local_motion | 367 | 363 | 60 | 60 |
| text_heavy | 55728 | 53534 | 12 | 13 |
| solid_fill | 19647 | 19821 | 24 | 25 |
| list_scroll | 34862 | 35423 | 18 | 18 |

**Takeaway:** Runs cleanly on MiSTer (`visual_ok=yes`, mid-run PNGs OK). **Slightly smaller binary**; **~30% longer** clean compile. Runtime deltas are within bench noise — no dramatic fps win on motion scenes; `full_motion` render ~5% lower in this run.

## Pin to crates.io 1.16.x (optional)

Remove `[patch.crates-io]` from `rust/Cargo.toml`, then `cargo update -p slint -p slint-build`.

## Bench

```bash
scripts/bench-toolchain.sh slint-master --skip-build --replace-label --device
```

**TSV label:** `slint-master`. **PNGs:** `slint-master-<scene>-fb.png`.
