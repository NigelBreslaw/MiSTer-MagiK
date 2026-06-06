# Cross-build profiles (`mister-magic-fb`)

Two **release** profiles separate fast host compiles from the binary we ship to the MiSTer.

| Profile | Command | LTO | CGUs | ARM flags | Clean build (~) | Binary (~) | Use |
|---------|---------|-----|------|-----------|-----------------|------------|-----|
| **`release`** | `build-arm.sh` or `--fast` | thin (`lto = true`) | 16 (default) | generic armv7 | ~3 min | ~1.65 MB | Daily Slint/UI iteration, quick deploy |
| **`release-device`** | `build-arm.sh --device` | fat | 1 | cortex-a9 + NEON | ~5 min | ~1.61 MB | SD card / bench / production |
| **`release-device-profile`** | `build-arm.sh --profile` | fat + debug | 1 | + frame pointers | ~5 min | ~4 MB | Profiling only (`MISTER_PROFILE`, `MISTER_PPROF`) |

Benchmark labels: **A0** ≈ `release`, **A3** ≈ `release-device` (see [`history/toolchain-bench/`](../history/toolchain-bench/)).

## Daily host checks

Routine host development should use the lightweight library checks instead of
plain `cargo test`, because the Slint UI binary intentionally cross-builds for
the MiSTer and can trip macOS AppKit code in Slint before reaching our tests.

```bash
scripts/dev-rust fmt       # cargo fmt --check
scripts/dev-rust fmt-fix   # cargo fmt
scripts/dev-rust test      # cargo test --lib --no-default-features
scripts/dev-rust check     # cargo check --lib --no-default-features
scripts/dev-rust build-ui  # rust/build-arm.sh --fast
```

The host-testable library contains pure catalog/controller/repeat logic. The
framebuffer, FPGA, Linux input loop, and Slint renderer stay in the binary target
behind Cargo feature `ui`.

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

# Video/audio benchmark build
rust/build-arm.sh --fast --video
# Builds/uses a minimal static FFmpeg under target/ffmpeg-minimal/armv7.
# Default media path on MiSTer: /media/fat/mister-magic/mslug3.mov

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

The minimal FFmpeg build enables only H.264-in-MOV/MP4 playback plus software
scaling and PCM stream discovery: `avcodec`, `avformat`, `avutil`, `swscale`,
H.264 decoder/parser, `pcm_s16le`, MOV demuxer, and file protocol.
`video_playback` writes 48 kHz stereo signed 16-bit PCM packets directly to
`/dev/MrAudio`, so AAC and swresample stay out of V1. avfilter, avdevice,
programs, and autodetected libraries are disabled.

`scripts/bench-toolchain.sh` calls `build-arm.sh` with no flags → **`release`** (matches historical A0 toolchain experiments unless you edit the script to pass `--device` for A3-style benches).

## CI

GitHub Actions builds the ARM frontend in `.github/workflows/rust-arm.yml`.

The matrix covers the local build modes that matter:

- `rust/build-arm.sh --fast`
- `rust/build-arm.sh --device`
- `rust/build-arm.sh --fast --video`
- `rust/build-arm.sh --device --video`

Each job installs pinned `cross` 0.2.5, uses `rust/Dockerfile.cross-armv7` via
`rust/Cross.toml`, caches Cargo registry/git data, caches the minimal FFmpeg tree
for video jobs, records `build/binary-size.tsv`, checks the ARM ELF dynamic
dependencies with `rust/scripts/check-arm-shared-libs.sh`, and uploads the binary
plus size TSV as artifacts.

The shared-library check intentionally fails if any `libav*`, `libswscale`, or
`libswresample` dependency appears. FFmpeg must stay statically linked from the
project-local minimal build.

## Config files

- **`Cargo.toml`** — `[profile.release]` vs `[profile.release-device]` (inherits release, overrides LTO/CGU).
- **Cargo feature `ui`** — enables Slint and `slint-build`; `build-arm.sh` passes it for every MiSTer binary build.
- **`.cargo/config.toml`** — sccache override only; no always-on `rustflags`.
- **`build-arm.sh`** — sets `RUSTFLAGS` for `release-device` only.

Prerequisite for NEON: `scripts/audit-mister.sh` → `A1 prerequisite: OK`.

## Slint version

**Default:** git `master` via `[patch.crates-io]` in `Cargo.toml` (currently 1.17.0 @ `9f5e4a49`). Comparison to crates.io 1.16: [`history/toolchain-bench/slint-master.md`](../history/toolchain-bench/slint-master.md).

After `cargo update`, confirm `Cargo.lock` still points at the intended git rev before shipping.

## cross-rs

Pin **0.2.5** from crates.io. Builds use the checked-in
`rust/Dockerfile.cross-armv7` through `rust/Cross.toml`; the image is based on
Ubuntu 20.04 to match the MiSTer glibc 2.31 runtime:

```bash
cargo install cross --version 0.2.5 --locked
```

Do not use `cargo install cross --git ...` unless you deliberately also change
the CI image/tooling assumptions.
