# Cross-build profiles (`mister-magik-fb`)

Two **release** profiles separate quick iteration compiles from the binary we ship to the MiSTer.

| Profile | Command | LTO | CGUs | ARM flags | Clean build (~) | Binary (~) | Use |
|---------|---------|-----|------|-----------|-----------------|------------|-----|
| **`release`** | `build-arm.sh` or `--fast` | thin (`lto = "thin"`) | 16 (default) | cortex-a9 | ~25s Rust edit loop | ~5.7 MB | Daily launcher iteration, quick deploy |
| **`release-fast-dev`** | `build-arm.sh --fast-dev` | off | 64 | cortex-a9 | ~10s Rust edit loop | ~10.7 MB | Fastest local Rust edit loop |
| **`release-incr`** | `build-arm.sh --incr` | thin | 16 (default) | cortex-a9 | ~18s Rust edit loop | ~5.6 MB | Faster local optimized edit loop |
| **`release-opt2`** | `build-arm.sh --opt2` | thin | 16 (default) | cortex-a9 | ~25s Rust edit loop | ~5.2 MB | Smaller local optimized smoke binary |
| **`release-opts`** | `build-arm.sh --opts` | thin | 16 (default) | cortex-a9 | ~24s Rust edit loop | ~4.2 MB | Smallest local optimized smoke binary |
| **`release-device`** | `build-arm.sh --device` | fat | 1 | cortex-a9 | ~5 min | ~1.61 MB | SD card / bench / production |
| **`release-device-profile`** | `build-arm.sh --profile` | fat + debug | 1 | cortex-a9 + frame pointers | ~5 min | ~4 MB | Profiling only (`MISTER_PROFILE`, `MISTER_PPROF`) |

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
scripts/dev-rust check-arm-lib       # ARM --lib check, no Slint/UI
scripts/dev-rust check-arm-ui        # ARM launcher/controller UI check
scripts/dev-rust check-ui            # alias: ARM launcher/controller UI check
scripts/dev-rust check-arm-ui-full   # ARM all-scenes UI check
scripts/dev-rust check-ui-full       # alias: ARM all-scenes UI check
scripts/dev-rust build-arm-debug     # ARM launcher/controller debug binary
scripts/dev-rust build-ui  # magik-gui/build-arm.sh --fast
```

The host-testable library contains pure catalog/controller/repeat logic. The
framebuffer, FPGA, Linux input loop, and Slint renderer stay in the binary target
behind Cargo feature `ui`.

Catalog and library-scan code lives in the path dependency
`magik-gui/catalog` (`mister-magik-catalog`). The main crate re-exports its
modules for compatibility, but catalog/media edits now have their own crate
boundary and benchmark state:

```bash
scripts/bench-debug-build.sh --scenario arm-check-launcher --state touch-rust-catalog --samples 3
```

Portable controller state lives in `src/input_state.rs`; Linux joystick polling
and raw js event decoding remain in `src/input.rs`. Keep navigation code
(`launcher.rs`, `setup_nav.rs`) on `input_state` types rather than importing the
Linux input module directly, so experiment 24 can move pure navigation into a
core crate later.

Recommended local loop:

```bash
# Pure Rust/catalog/controller edits.
scripts/dev-rust check

# ARM-only pure library confidence.
scripts/dev-rust check-arm-lib

# Slint launcher/controller edits without producing a deploy binary.
scripts/dev-rust check-ui

# Before changing benchmark scenes, video, or generated all-scene UI wiring.
scripts/dev-rust check-ui-full

# Only when a device binary is needed.
magik-gui/build-arm.sh --incr     # optimized, faster edit-loop deploy
magik-gui/build-arm.sh --fast-dev # fastest Rust edit-loop deploy, larger binary
```

Measured check-loop anchors from
[`compile-time-experiments-20260609.md`](../history/toolchain-bench/compile-time-experiments-20260609.md):
warm no-op `check-arm-ui` is about `2.9s`; touching Rust UI code is about
`3.6s` after the generated UI crate split; touching launcher/shared Slint files
is about `33s`, dominated by the `mister-magik-ui` build script / Slint codegen
rather than Rust type-checking.

For debug-time build measurements, use:

```bash
scripts/bench-debug-build.sh --scenario arm-check-launcher --samples 3
scripts/bench-debug-build.sh --scenario all --samples 3 --state touch-rust-bin
```

The benchmark writes `build/debug-build-bench.tsv` with wall time, Cargo total
time, and the largest `mister-magik-fb` timing units. `check-arm-ui` uses
`MISTER_UI_BUILD_SCOPE=launcher`, so it compiles only the launcher, controller
test, and their required shared Slint modules. `check-arm-arcade-ui` uses
`MISTER_UI_BUILD_SCOPE=arcade` and keeps the standalone arcade page. `check-arm-ui-full`,
`build-arm-debug-full`, and `build-arm.sh --all-scenes` enable the
`bench-scenes` feature and compile every benchmark scene.

Slint code generation lives in the `mister-magik-ui` path crate under
`ui-generated/`. The main binary still drives the runtime, but ordinary Rust
edits can now reuse the generated UI crate instead of embedding all generated
Slint modules directly in `ui_runner.rs`.

The UI crate build script keeps a content fingerprint for the selected Slint,
font, and icon inputs. If a file's mtime changes but the bytes and relevant
build settings are identical, it reuses the generated files already in `OUT_DIR`
instead of rerunning Slint codegen; real source-content changes still regenerate
the UI modules.

Launcher-scope builds intentionally omit standalone `demo` and `arcade_page`
scene code to keep local optimized builds small. Use `--ui-scope arcade` for
standalone arcade-page work and `--all-scenes` for benchmark/demo coverage.

Compile-time experiment tracking lives in
[`history/toolchain-bench/compile-time-experiments-20260609.md`](../history/toolchain-bench/compile-time-experiments-20260609.md).
The policy is: commit the harness and curated reports, merge only winning
changes, and summarize failed experiment branches rather than merging them.

## Commands

```bash
# Fast — default for bare build-arm.sh, still Cortex-A9 tuned.
# Builds launcher-scoped Slint by default for local iteration.
magik-gui/build-arm.sh
# → target/armv7-unknown-linux-gnueabihf/release/mister-magik-fb

# Fast but compile every Slint benchmark scene (adds feature `bench-scenes`).
magik-gui/build-arm.sh --fast --all-scenes

# Experimental local profile: no LTO, high CGUs, incremental.
magik-gui/build-arm.sh --fast-dev

# Local optimized profile with incremental reuse.
magik-gui/build-arm.sh --incr

# Local optimized profile: opt-level=2 + thin LTO, smaller than release.
magik-gui/build-arm.sh --opt2

# Local size-optimized profile: opt-level=s + thin LTO.
magik-gui/build-arm.sh --opts

# Experimental native Apple-Silicon Docker path: linux/arm64 container,
# linux/aarch64 Rust host toolchain, armv7 target.
magik-gui/build-arm64-docker.sh --opts

# Full MiSTer release (fat LTO + Cortex-A9)
magik-gui/build-arm.sh --device
# → target/.../release-device/mister-magik-fb

# Profiling build (symbols, pprof feature — do not ship)
magik-gui/build-arm.sh --profile
# → target/.../release-device-profile/mister-magik-fb
# Run on device: scripts/cpu-flamegraph-scene.sh full_motion 10 FM-CPU

# Video/audio benchmark build
magik-gui/build-arm.sh --fast --video
# Builds/uses a minimal static FFmpeg under target/ffmpeg-minimal/armv7.
# Video builds force all-scenes UI scope because video_playback.slint is a bench scene.
# Default media path on MiSTer: /media/fat/mister-magik/mslug3.mov

# Deploy (default = release-device)
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh

# Deploy after a fast build (same path on device, thin LTO + Cortex-A9)
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh --fast

# Deploy a fast build with every Slint scene included.
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh --fast --all-scenes
```

Every `build-arm.sh` run prints the binary size and appends a local row to
`build/binary-size.tsv` with profile, features, bytes, and delta versus the prior
matching build. The file is intentionally gitignored; formal benchmark byte
history remains in `history/toolchain-bench/results.tsv`.

## Size analysis

For subsystem-level size work, build an unstripped diagnostic binary and generate
symbol reports:

```bash
magik-gui/build-arm.sh --profile
magik-gui/scripts/analyze-binary-size.sh
```

Reports are written to `build/binary-size-analysis/`:

- `groups.tsv` — rough grouping: FFmpeg/video, Slint/generated UI, fonts/text,
  SQLite/catalog, PNG/preview, launcher/input/fb, other.
- `top-symbols.tsv` — largest symbols.
- `nm-symbols.tsv` — raw symbol-size listing.

## CPU flamegraphs

`scripts/cpu-flamegraph-scene.sh SCENE [SECS] [LABEL]` builds the profiling
binary, deploys it, runs a deterministic `cpu-profile-smoke 3` check, then runs
the requested scene with `MISTER_PPROF=1`.

Artifacts are written to `build/cpu-flamegraphs/`:

- `<label>-cpu-smoke.log` / `<label>-cpu-smoke.svg` — profiler smoke proof.
- `<label>-cpu-profile.log` / `<label>-cpu.svg` — scene run and flamegraph.

This path uses in-process `SIGPROF` / `ITIMER_PROF` sampling through `pprof`; it
does not require a MiSTer-side `perf` binary, and `perf_event_paranoid` is not the
primary control. If the log says `0 sample hits`, the sampling timer did not
produce usable profiler interrupts; run the smoke command directly on the MiSTer:

```bash
MISTER_PPROF=1 MISTER_PPROF_OUT=/tmp/smoke.svg \
  /media/fat/mister-magik/mister-magik-fb cpu-profile-smoke 3
```

## Minimal FFmpeg

`--video` builds do not use the broad `ffmpeg-the-third` FFmpeg builder. Instead,
`build-arm.sh` runs `magik-gui/scripts/build-minimal-ffmpeg.sh`, then passes
`FFMPEG_DIR=/project/target/ffmpeg-minimal/armv7/dist` into `cross`.

The minimal FFmpeg build enables only H.264-in-MOV/MP4 playback plus software
scaling and PCM stream discovery: `avcodec`, `avformat`, `avutil`, `swscale`,
H.264 decoder/parser, `pcm_s16le`, MOV demuxer, and file protocol.
`video_playback` writes 48 kHz stereo signed 16-bit PCM packets directly to
`/dev/MrAudio`, so AAC and swresample stay out of V1. avfilter, avdevice,
programs, and autodetected libraries are disabled.

`scripts/bench-toolchain.sh` calls `build-arm.sh` with no flags → **`release`**. Current release builds are Cortex-A9 tuned; older A0 history used the same profile shape before that fast-path tuning was added.

## CI

GitHub Actions builds the ARM frontend in `.github/workflows/rust-arm.yml`.

The matrix covers the local build modes that matter:

- `magik-gui/build-arm.sh --fast`
- `magik-gui/build-arm.sh --device`
- `magik-gui/build-arm.sh --fast --video`
- `magik-gui/build-arm.sh --device --video`

Each job installs pinned `cross` 0.2.5, uses `magik-gui/Dockerfile.cross-armv7` via
`magik-gui/Cross.toml`, caches Cargo registry/git data, caches the minimal FFmpeg tree
for video jobs, records `build/binary-size.tsv`, checks the ARM ELF dynamic
dependencies with `magik-gui/scripts/check-arm-shared-libs.sh`, and uploads the binary
plus size TSV as artifacts.

The shared-library check intentionally fails if any `libav*`, `libswscale`, or
`libswresample` dependency appears. FFmpeg must stay statically linked from the
project-local minimal build.

## Config files

- **`Cargo.toml`** — `[profile.release]` vs `[profile.release-device]` (inherits release, overrides LTO/CGU).
- **Cargo feature `ui`** — enables Slint and the `mister-magik-ui` generated UI
  crate; `build-arm.sh` passes it for every MiSTer binary build.
- **Cargo feature `bench-scenes`** — enables generated Slint benchmark scenes
  and effect-bench Slint overlays. Ordinary UI builds omit it; `--all-scenes`
  and `scripts/bench-toolchain.sh` opt in.
- **`MISTER_UI_BUILD_SCOPE` / `--ui-scope`** — `release` defaults to
  `launcher` for fast local iteration; `release-device`, profiling, `--video`,
  and `--all-scenes` use `all`.
- **root `.cargo/config.toml` + `.cargo/config.toml`** — disable any inherited
  compiler wrapper such as sccache; no always-on `rustflags`.
- **`build-arm.sh`** — sets Cortex-A9 `RUSTFLAGS` for every optimized ARM build; profiling also adds frame pointers.

Prerequisite for Cortex-A9 tuning: `scripts/audit-mister.sh` → `A1 prerequisite: OK`.

## Slint version

**Default:** git `master` via `[patch.crates-io]` in `Cargo.toml` (currently 1.17.0 @ `9f5e4a49`). Comparison to crates.io 1.16: [`history/toolchain-bench/slint-master.md`](../history/toolchain-bench/slint-master.md).

After `cargo update`, confirm `Cargo.lock` still points at the intended git rev before shipping.

## cross-rs

Pin **0.2.5** from crates.io. Builds use the checked-in
`magik-gui/Dockerfile.cross-armv7` through `magik-gui/Cross.toml`; the image is based on
Ubuntu 20.04 to match the MiSTer glibc 2.31 runtime:

```bash
cargo install cross --version 0.2.5 --locked
```

Do not use `cargo install cross --git ...` unless you deliberately also change
the CI image/tooling assumptions.

### Native ARM64 Docker experiment

`magik-gui/build-arm64-docker.sh` is an opt-in Apple-Silicon experiment for
building inside a `linux/arm64` Docker image instead of running the normal
amd64 `cross` image through emulation. It reuses `Dockerfile.cross-armv7`, mounts
a Linux/aarch64 Rust toolchain, and builds the `armv7-unknown-linux-gnueabihf`
target directly.

One-time setup:

```bash
rustup toolchain add stable-aarch64-unknown-linux-gnu --profile minimal --force-non-host
rustup target add armv7-unknown-linux-gnueabihf --toolchain stable-aarch64-unknown-linux-gnu
```

Example:

```bash
magik-gui/build-arm64-docker.sh --opts
# → /private/tmp/mister-magik-arm64-target/armv7-unknown-linux-gnueabihf/release-opts/mister-magik-fb
```

This path exists because `cross` 0.2.5 currently mounts an x86_64 Linux Rust
sysroot on macOS even when `DOCKER_DEFAULT_PLATFORM=linux/arm64`; the arm64
container then fails before Rust starts because it sees an x86_64 dynamic
loader. The plain Docker path avoids that mismatch and has produced a valid
ARMv7 MiSTer binary. Keep it opt-in until repeated timing runs show it is better
than the standard `build-arm.sh` path.
