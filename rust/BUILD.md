# Cross-build profiles (`mister-magic-fb`)

Two **release** profiles separate fast host compiles from the binary we ship to the MiSTer.

| Profile | Command | LTO | CGUs | ARM flags | Clean build (~) | Binary (~) | Use |
|---------|---------|-----|------|-----------|-----------------|------------|-----|
| **`release`** | `build-arm.sh` or `--fast` | thin (`lto = true`) | 16 (default) | generic armv7 | ~3 min | ~1.65 MB | Daily Slint/UI iteration, quick deploy |
| **`release-device`** | `build-arm.sh --device` | fat | 1 | cortex-a9 + NEON | ~5 min | ~1.61 MB | SD card / bench / production |
| **`release-device-profile`** | `build-arm.sh --profile` | fat + debug | 1 | + frame pointers | ~5 min | ~4 MB | Profiling only (`MISTER_PROFILE`, `MISTER_PPROF`) |

Benchmark labels: **A0** ≈ `release`, **A3** ≈ `release-device` (see [`history/toolchain-bench/`](../history/toolchain-bench/)).

## Commands

```bash
# Fast — default for bare build-arm.sh
rust/build-arm.sh
# → target/armv7-unknown-linux-gnueabihf/release/mister-magic-fb

# Full MiSTer release (fat LTO + NEON via RUSTFLAGS)
rust/build-arm.sh --device
# → target/.../release-device/mister-magic-fb

# Profiling build (symbols, pprof feature — do not ship)
rust/build-arm.sh --profile
# → target/.../release-device-profile/mister-magic-fb
# Run on device: scripts/profile-scene.sh full_motion 30

# Video benchmark build
rust/build-arm.sh --fast --video
# Builds/uses a minimal static FFmpeg under target/ffmpeg-minimal/armv7.

# Deploy (default = release-device)
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh

# Deploy after a fast build (same path on device, larger binary)
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh --fast
```

Every `build-arm.sh` run prints the binary size and appends a local row to
`build/binary-size.tsv` with profile, features, bytes, and delta versus the prior
matching build. The file is intentionally gitignored; formal benchmark byte
history remains in `history/toolchain-bench/results.tsv`.

## Size analysis

For subsystem-level size work, build an unstripped diagnostic binary and generate
symbol reports:

```bash
rust/build-arm.sh --profile
rust/scripts/analyze-binary-size.sh
```

Reports are written to `build/binary-size-analysis/`:

- `groups.tsv` — rough grouping: FFmpeg/video, Slint/generated UI, fonts/text,
  SQLite/catalog, PNG/preview, launcher/input/fb, other.
- `top-symbols.tsv` — largest symbols.
- `nm-symbols.tsv` — raw symbol-size listing.

## Minimal FFmpeg

`--video` builds do not use the broad `ffmpeg-the-third` FFmpeg builder. Instead,
`build-arm.sh` runs `rust/scripts/build-minimal-ffmpeg.sh`, then passes
`FFMPEG_DIR=/project/target/ffmpeg-minimal/armv7/dist` into `cross`.

The minimal FFmpeg build enables only H.264-in-MP4 playback plus software scaling:
`avcodec`, `avformat`, `avutil`, `swscale`, H.264 decoder/parser, MOV demuxer,
and file protocol. Audio, swresample, avfilter, avdevice, programs, docs, and
autodetected libraries are disabled.

`scripts/bench-toolchain.sh` calls `build-arm.sh` with no flags → **`release`** (matches historical A0 toolchain experiments unless you edit the script to pass `--device` for A3-style benches).

## Config files

- **`Cargo.toml`** — `[profile.release]` vs `[profile.release-device]` (inherits release, overrides LTO/CGU).
- **`.cargo/config.toml`** — sccache override only; no always-on `rustflags`.
- **`build-arm.sh`** — sets `RUSTFLAGS` for `release-device` only.

Prerequisite for NEON: `scripts/audit-mister.sh` → `A1 prerequisite: OK`.

## Slint version

**Default:** git `master` via `[patch.crates-io]` in `Cargo.toml` (currently 1.17.0 @ `9f5e4a49`). Comparison to crates.io 1.16: [`history/toolchain-bench/slint-master.md`](../history/toolchain-bench/slint-master.md).

After `cargo update`, confirm `Cargo.lock` still points at the intended git rev before shipping.

## cross-rs

Pin **0.2.5** from crates.io (matches MiSTer glibc 2.31 via our link setup):

```bash
cargo install cross --version 0.2.5 --locked
```

Docker image: `ghcr.io/cross-rs/armv7-unknown-linux-gnueabihf:0.2.5`. Do not use `cargo install cross --git …` (that pulls the `:main` image).
