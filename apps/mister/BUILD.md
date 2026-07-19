# Cross-build profiles (`mister-magik-fb`)

`release-device` is the normal MiSTer build.

| Profile | Command | LTO | CGUs | ARM flags | Clean build (~) | Binary (~) | Use |
|---------|---------|-----|------|-----------|-----------------|------------|-----|
| **`release-device`** | `build-arm.sh` or `--device` | fat | 1 | cortex-a9 + neon | ~4 min | ~9.5 MiB current UI | SD card / bench / production |
| **`release`** | `build-arm.sh --fast` | thin | 16 (Cargo default) | cortex-a9 + neon | ~2 min | ~11.3 MB current UI | Optimized daily deploy |
| **`release-device-profile`** | `build-arm.sh --profile` | fat + debug | 1 | cortex-a9 + neon + frame pointers | ~5 min | ~4 MB | Profiling only (`MISTER_PROFILE`, `MISTER_PPROF`) |

Historical benchmark labels: **A0** ≈ old thin-LTO `release`, **A3** ≈ `release-device` (see [`history/toolchain-bench/`](../history/toolchain-bench/)).

## Daily host checks

Routine host development should use the lightweight library checks by default.
The macOS host can also compile the UI feature as a smoke test: Cargo enables
Slint `std` only for macOS so Slint's AppKit system tray support compiles, while
ARM/MiSTer targets keep the embedded no-std Slint configuration.

```bash
scripts/dev-rust fmt       # cargo fmt --check
scripts/dev-rust fmt-fix   # cargo fmt
scripts/dev-rust test      # cargo test --lib --no-default-features
scripts/dev-rust check     # cargo check --lib --no-default-features
cargo test --manifest-path apps/mister/Cargo.toml --features ui --no-default-features
scripts/dev-rust check-arm-lib       # ARM --lib check, no Slint/UI
scripts/dev-rust check-arm-ui        # ARM launcher/controller UI check
scripts/dev-rust check-ui            # alias: ARM launcher/controller UI check
scripts/dev-rust check-arm-ui-full   # ARM all-scenes UI check
scripts/dev-rust check-ui-full       # alias: ARM all-scenes UI check
scripts/dev-rust build-arm-debug     # ARM launcher/controller debug binary
scripts/dev-rust build-ui  # apps/mister/build-arm.sh
```

## Local commit hook

The repo tracks a pre-commit hook in `.githooks/pre-commit`, but Git hook
configuration is clone-local. Enable it on a Mac checkout with:

```bash
git config core.hooksPath .githooks
```

The hook runs `scripts/validate affected`. It always checks formatting and cheap
repository contracts, then selects Rust tests, Clippy, and the production UI
check from the staged paths. Catalog checks run only for catalog or shared-build
inputs. Use `scripts/validate affected --paths-file FILE` to reproduce routing
without changing the index, and `scripts/validate full-host` for the complete CI
assurance gate. There is deliberately no pre-push hook.

The host-testable library contains pure catalog/controller/repeat logic. The
framebuffer, FPGA, Linux input loop, and Slint renderer stay in the binary target
behind Cargo feature `ui`; use the explicit `cargo test --features ui
--no-default-features` smoke when you want host coverage for the UI binary target
without producing a deployable macOS app.

Catalog and library-scan code lives in the path dependency
`crates/catalog` (`mister-magik-catalog`). The main crate re-exports its
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
apps/mister/build-arm.sh            # release-device
```

On Apple Silicon these commands use the native Apple-container backend; Linux
and CI use `cross check`. Check mode shares the production target, image,
FFmpeg, Cortex-A9 flags, and caches, but never mirrors a binary, writes a build
receipt, records binary size, deploys, or contacts a MiSTer.

Historical measured check-loop anchors from
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
`MISTER_UI_BUILD_SCOPE=arcade` and keeps the launcher-backed arcade screen. `check-arm-ui-full`,
`build-arm-debug-full`, and `build-arm.sh --all-scenes` enable the
`experiments` feature, which includes generated benchmark scenes and the
experimental effect scenes.

Slint code generation lives in the `mister-magik-ui` path crate under
`ui-generated/`. The main binary still drives the runtime, but ordinary Rust
edits can now reuse the generated UI crate instead of embedding all generated
Slint modules directly in `ui_runner.rs`.

The UI crate build script keeps a content fingerprint for the selected Slint,
font, and icon inputs. If a file's mtime changes but the bytes and relevant
build settings are identical, it reuses the generated files already in `OUT_DIR`
instead of rerunning Slint codegen; real source-content changes still regenerate
the UI modules.

Launcher-scope builds intentionally omit standalone `demo` scene code to keep
local optimized builds small. Use `--ui-scope arcade` for real arcade-screen
work and `--all-scenes` for benchmark/demo coverage.

Compile-time experiment tracking lives in
[`history/toolchain-bench/compile-time-experiments-20260609.md`](../history/toolchain-bench/compile-time-experiments-20260609.md).
The policy is: commit the harness and curated reports, merge only winning
changes, and summarize failed experiment branches rather than merging them.

## Commands

```bash
# Full MiSTer release (fat LTO + Cortex-A9)
apps/mister/build-arm.sh
# → target/armv7-unknown-linux-gnueabihf/release-device/mister-magik-fb
# On Apple Silicon this uses Apple's container runtime by default.
# Set MISTER_ARM_BUILD_BACKEND=cross to force the CI/Linux cross-rs backend.

# Compile Slint benchmark scenes and experimental effect scenes.
apps/mister/build-arm.sh --all-scenes

# Compile experimental effect scenes explicitly.
apps/mister/build-arm.sh --experiments

# Explicit spelling for the same release-device build.
apps/mister/build-arm.sh --device
# → target/.../release-device/mister-magik-fb

# Fully optimized daily build (thin LTO + parallel codegen).
apps/mister/build-arm.sh --fast
# → target/.../release/mister-magik-fb
# Defaults to launcher UI scope; pass --ui-scope or --all-scenes explicitly
# when a broader generated-UI surface is required.

# Profiling build (symbols, pprof feature — do not ship)
apps/mister/build-arm.sh --profile
# → target/.../release-device-profile/mister-magik-fb
# Run on device: scripts/cpu-flamegraph-scene.sh video_playback 10 VIDEO-CPU

# Every UI build includes production video/audio support and builds/uses a
# minimal static FFmpeg under target/ffmpeg-minimal/armv7.
# Keeps only the production paths: direct blit, source-size RGB565 conversion,
# and optional 2x RGB565 expansion for already-half-size assets.
# Default media folder on MiSTer: /media/fat/mister-magik/video-snaps/neogeo

# Deploy the runtime binary only (default = release-device). The command
# preflights the dev platform and atomically rebinds only its GUI hash.
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh

# Deploy an experiment-enabled binary for lab scripts.
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/deploy-rust.sh --all-scenes
```

Every `build-arm.sh` run prints the binary size and appends a local row to
`build/binary-size.tsv` with profile, features, bytes, and delta versus the prior
matching build. The file is intentionally gitignored; formal benchmark byte
history remains in `history/toolchain-bench/results.tsv`.

After moving bench commands behind `bench-tools` and removing the standalone
`audio-tone` path, the measured ARM
`release-device`/`ui` binary is 5,955,500 bytes. The `ui,bench-tools` variant is
6,078,388 bytes, so the quarantined benchmark surface costs 122,888 bytes.

## Size analysis

For subsystem-level size work, build an unstripped diagnostic binary and generate
symbol reports:

```bash
apps/mister/build-arm.sh --profile
apps/mister/scripts/analyze-binary-size.sh
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

Generate the alpha release's ARM `ui` Rust dependency inventory and the
bundled FFmpeg/font notices after changing `Cargo.lock` or release features:

```bash
python3 scripts/release/packaging/generate-third-party-licenses.py
```

The generator follows only normal runtime dependencies for
`armv7-unknown-linux-gnueabihf`, deduplicates identical legal texts, and writes
the release artifacts under `apps/mister/licenses/` for inclusion beside the
distributed binary. The launcher keeps its license screen concise; the complete
runtime dependency inventory is available in the distribution ZIP and Docs.

Stage the alpha binary and all sidecar license files together with:

```bash
scripts/package-alpha-release.sh [OUT_DIR]
```

Pass `--skip-build` as the second argument only when the current ARM
`ui` release binary has already been built and verified.

UI builds do not use a crate-managed broad FFmpeg builder. Instead, `build-arm.sh`
runs `apps/mister/scripts/build-minimal-ffmpeg.sh`, then passes the
resulting static libraries into the selected ARM Rust build backend. The cross-rs
backend sees FFmpeg under `/target/ffmpeg-minimal/armv7/dist`; the local Apple
container backend sees the same host cache under
`/project/target/ffmpeg-minimal/armv7/dist`.

The production minimal FFmpeg build pins upstream FFmpeg 8.1.2 and enables only
H.264-in-MOV/MP4 playback plus audio decode/resampling for MiSTer video snaps:
`avcodec`, `avformat`, `avutil`, `swresample`, H.264 decoder/parser, AAC
decoder/parser, `pcm_s16le`, MOV demuxer, and file protocol.
`video_playback` decodes audio and converts it to 48 kHz stereo signed 16-bit
PCM for `/dev/MrAudio`. FFmpeg is built for `--cpu=cortex-a9` with
`-mfpu=neon-vfpv3` and runtime CPU detection, and the build script fails if the
generated config or `libavcodec.a` no longer contains the expected ARM/NEON H.264
decode support.

The project-owned video presentation code is plain Rust RGB565 conversion. It
supports `MISTER_VIDEO_SCALE=source` for native-size presentation and
`MISTER_VIDEO_SCALE=2x` for the 320x240-to-640x480 pixel-doubling path. There is
no production FFmpeg swscale, Slint-image upload, decoder-thread, or alternate
conversion backend.
avfilter, avdevice, swscale, programs, and autodetected libraries are disabled.
The checked-in ARMv7 image includes both C and C++ cross toolchains so FFmpeg and
any C++-probing native crates see the same compiler family locally and in CI.

`scripts/bench-toolchain.sh` calls `build-arm.sh` with no flags → **`release-device`**. Older A0 history used the removed thin-LTO fast path.

## CI

GitHub Actions builds the ARM frontend in `.github/workflows/rust-arm.yml`.

CI builds the single production frontend with `apps/mister/build-arm.sh --device`.

CI runs on Linux, so `build-arm.sh` uses pinned `cross` 0.2.5 there. Each job
pulls the content-versioned GHCR image selected by `apps/mister/Cross.toml`; the
image is built from `apps/mister/Dockerfile.cross-armv7`. CI caches Cargo
registry/git data, caches the minimal FFmpeg tree, records
`build/binary-size.tsv`, checks the ARM ELF dynamic dependencies with
`apps/mister/scripts/check-arm-shared-libs.sh`, and uploads the binary plus size
TSV as artifacts.

Host, ARM, agent, distribution, and Main caches use the schema-v2 identities
from `scripts/checks/ci-cache-identity.py`. Binary caches are scoped by runner OS and
architecture plus the Rust/cross ABI; `incremental/` directories are excluded
from transfer. `scripts/tests/test-ci-cache-contract.py` is the policy gate for every
workflow cache key.

The manual **Cross image** workflow is main-only and publishes each
Dockerfile-hash tag exactly once. When changing `Dockerfile.cross-armv7`, update
the `ubuntu20-<first-12-SHA256-characters>` tag in `Cross.toml`, merge that
change to `main`, then dispatch **Cross image**. Re-dispatching an existing tag
fails rather than replacing the image behind existing CI cache identities.

The manual FPGA workflow does not use GitHub Actions cache for Quartus. Its
installed Quartus 17.0/Cyclone V runtime is stored as a private,
content-addressed `tar.zst` in the dedicated R2 Standard bucket
`mister-magik-ci-cache`; no installer payload is uploaded. Configure these
repository Actions values before dispatching the workflow:

```text
variable  QUARTUS_R2_ACCOUNT_ID
variable  QUARTUS_R2_BUCKET=mister-magik-ci-cache
secret    QUARTUS_R2_READ_ACCESS_KEY_ID
secret    QUARTUS_R2_READ_SECRET_ACCESS_KEY
secret    QUARTUS_R2_WRITE_ACCESS_KEY_ID
secret    QUARTUS_R2_WRITE_SECRET_ACCESS_KEY
```

Create separate, bucket-scoped R2 S3 tokens: Object Read only for the read
pair and Object Read & Write for the write pair. The bucket must remain private
with no custom domain or `r2.dev` exposure. `scripts/quartus-r2-cache.sh`
publishes the checksum manifest last, so an interrupted multipart upload is a
cache miss rather than a corrupt hit.

The shared-library check intentionally fails if any `libav*`, `libswscale`, or
`libswresample` dependency appears. FFmpeg must stay statically linked from the
project-local minimal build.

## Config files

- **`Cargo.toml`** — `[profile.release]` vs `[profile.release-device]` (inherits release, overrides LTO/CGU).
- **Cargo feature `ui`** — enables Slint, the `mister-magik-ui` generated UI
  crate, and mandatory ARM/Linux FFmpeg video support; `build-arm.sh` passes it
  for every MiSTer frontend build.
- **Cargo feature `bench-scenes`** — enables generated Slint benchmark scenes.
- **Cargo feature `bench-tools`** — enables on-device benchmark commands and
  launcher benchmark automation that are intentionally absent from production
  `ui` builds.
- **Cargo feature `experiments`** — includes `bench-scenes` and enables
  experimental effect pickers, expanded preview transitions, and `effect-bench`.
  Ordinary UI builds omit it; `--all-scenes`, `--experiments`, and
  `scripts/bench-toolchain.sh` opt in.
- **Cargo feature `diagnostics`** — enables low-level runtime probes that are
  intentionally absent from the production command surface.
- **`MISTER_ARM_BUILD_BACKEND`** — local Apple-Silicon builds default to
  `apple-container`; set `cross` to force the CI/Linux Docker backend.
- **`MISTER_UI_BUILD_SCOPE` / `--ui-scope`** — controls the generated UI build
  scope for launcher-oriented builds. It is not a hard product-surface split:
  `launcher` and `arcade` both build the launcher shell, while `all` opts into
  lab scene generation when matching features are enabled.
- **`apps/mister/.cargo/config.toml`** — keeps direct Cargo invocations on the
  normal compiler path; no always-on `rustflags`.
- **`build-arm.sh`** — sets Cortex-A9 `RUSTFLAGS` for every optimized ARM build;
  profiling also adds frame pointers. NEON-specific video acceleration belongs
  to the FFmpeg build, where the generated config is checked explicitly.

Prerequisite for Cortex-A9 tuning: `scripts/audit-mister.sh` → `A1 prerequisite: OK`.

## Slint version

**Default:** git `master` via `[patch.crates-io]` in `Cargo.toml`; `Cargo.lock`
records the exact revision used by the current workspace. Comparison to crates.io
1.16: [`history/toolchain-bench/slint-master.md`](../history/toolchain-bench/slint-master.md).

After `cargo update`, confirm `Cargo.lock` still points at the intended git rev before shipping.

## Local Apple-Container Backend

On Apple Silicon, `apps/mister/build-arm.sh` automatically delegates to
`apps/mister/build-arm64-apple-container.sh`. That path runs a native `linux/arm64`
container through Apple's Virtualization Framework, mounts a Linux/aarch64 Rust
toolchain, builds the ARMv7 target directly, then mirrors the final binary back
to the normal `apps/mister/target/armv7-unknown-linux-gnueabihf/<profile>/` path so
deploy and benchmark scripts do not need special cases.

Local ARM helper scripts also prefer Apple's `container` runtime on Apple
Silicon. This includes `scripts/build-mister-agent.sh`,
`apps/mister/scripts/check-arm-shared-libs.sh`, and
`apps/mister/scripts/analyze-binary-size.sh`. Docker/cross remains the Linux and
CI backend, and can be forced locally only for comparison with
`MISTER_ARM_BUILD_BACKEND=cross`.

One-time setup:

```bash
rustup toolchain add stable-aarch64-unknown-linux-gnu --profile minimal --force-non-host
rustup target add armv7-unknown-linux-gnueabihf --toolchain stable-aarch64-unknown-linux-gnu
container system start
container builder start --cpus "$(getconf _NPROCESSORS_ONLN)" --memory 8g
```

Useful knobs:

- `MISTER_ARM_BUILD_BACKEND=cross` — force the old cross-rs/Docker backend.
- `MISTER_APPLE_CONTAINER_TARGET_DIR=/path` — move the Apple-container Cargo
  target cache; default is `/private/tmp/mister-magik-apple-container-target`.
- `MISTER_APPLE_CONTAINER_MIRROR_TARGET_DIR=/path` — mirror artifacts somewhere
  other than `apps/mister/target`.
- `MISTER_APPLE_CONTAINER_REBUILD_IMAGE=1` or `--rebuild-image` — force a
  rebuild of the Linux/arm64 cross image. Otherwise the script rebuilds it only
  when the image is missing or `Dockerfile.cross-armv7` changes.

The Apple builder VM must be restarted with the full CPU/memory allocation before
the build script can use it. Builds inspect the running builder allocation and
warn with the exact restart commands when it is below the detected host CPU
count or 8 GiB; they never restart the shared builder automatically.

## cross-rs

CI and non-Apple local builds use **0.2.5** from crates.io. CI uses the
content-versioned GHCR image named in `apps/mister/Cross.toml`; it is built from
the checked-in `apps/mister/Dockerfile.cross-armv7`. The image is based on Ubuntu
20.04 to match the MiSTer glibc 2.31 runtime:

```bash
cargo install cross --version 0.2.5 --locked
```

Do not use `cargo install cross --git ...` unless you deliberately also change
the CI image/tooling assumptions.

Legacy Apple-Silicon Docker and sccache builder variants were retired. Use the
Apple-container backend for local Apple Silicon builds, or set
`MISTER_ARM_BUILD_BACKEND=cross` only when deliberately comparing with the
Linux/CI cross-rs path.
