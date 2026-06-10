# Compile-time experiments (2026-06-09)

Goal: reduce local Apple-Silicon Mac → Docker → ARMv7 cross-build time for
`magik-gui` while keeping Slint on git `master` and preserving the production
`release-device` path until a replacement is proven.

## Artifact policy

- **Merge winners only.** Experiment branches may be messy while measuring, but
  mainline should receive only the harness, curated reports, docs, and accepted
  improvements.
- **Document everything.** Every experiment gets an entry with hypothesis,
  branch, changed files, exact commands, timing deltas, binary-size impact,
  smoke-test result, and decision.
- **Keep raw data out of git.** Raw logs, Cargo timing HTML, and local TSV rows
  stay under gitignored `build/`. Summaries and decisions go here.
- **Protect production.** `release-device` remains the ship profile unless an
  experiment proves a safe replacement with shared-library and device smoke
  evidence.

## Measurement harness

Tracked script: `scripts/bench-debug-build.sh`.

Raw output:

- `build/debug-build-bench.tsv`
- `build/debug-build-logs/*.log`

Representative commands:

```bash
scripts/bench-debug-build.sh --scenario arm-check-launcher --state noop-warm --samples 5
scripts/bench-debug-build.sh --scenario all --state touch-rust-bin --samples 3
scripts/bench-debug-build.sh --scenario build-ui-fast --state touch-slint-launcher --samples 3
```

For comparable runs, keep Docker Desktop CPU/memory settings fixed, use
`--locked`, and record whether the worktree is dirty. Baseline runs must keep
`RUSTC_WRAPPER=""`; any sccache work is isolated to its own experiment.

## Research anchors

- Rust Performance Book, "Compile Times":
  <https://nnethercote.github.io/perf-book/compile-times.html>. This backs the
  use of `cargo build --timings`, graph-parallelism inspection before crate
  splits, macro/codegen scrutiny, and the final `cargo llvm-lines` pass.
- Cargo Book, "Profiles":
  <https://doc.rust-lang.org/cargo/reference/profiles.html>. This backs the
  profile experiments around `opt-level`, `lto`, `strip`, `incremental`, custom
  profiles, and `codegen-units`.
- Mozilla `sccache`: <https://github.com/mozilla/sccache>. Any compiler-cache
  trial must stay isolated because this repo deliberately clears inherited
  `RUSTC_WRAPPER` for reproducibility.
- `cargo-chef`: <https://github.com/LukeMathWalker/cargo-chef>. This informs
  the Docker dependency prewarm experiment, while recognizing that local
  bind-mounted Cargo caches may behave differently from CI image-layer caches.
- `mold`: <https://github.com/rui314/mold>. This is the prior art for the mold
  linker experiment; LLD is tested first because it is easier to install in the
  current Ubuntu 20.04 cross image.

## Experiment queue

Status values: `todo`, `running`, `accepted`, `rejected`, `blocked`.

| # | Experiment | Status | Merge rule |
|---|------------|--------|------------|
| 1 | Baseline harness | accepted | Merge harness + docs. |
| 2 | Local launcher scope by default | accepted | Merge only if launcher builds shrink without hiding full-scene path. |
| 3 | Bench scenes feature split | accepted | Merge only if normal UI builds avoid bench-scene Slint code. |
| 4 | Video feature isolation | accepted | Merge only if normal UI builds avoid video crates/codegen. |
| 5 | Add `release-fast-dev` profile | accepted | Merge only if faster and device smoke passes. |
| 6 | No-LTO fast build | rejected | Merge only if local profile improves wall time. |
| 7 | Thin-LTO fast build | accepted | Merge only if runtime benefit justifies time. |
| 8 | `opt-level=2` | accepted | Merge only in local profile if 60fps smoke passes. |
| 9 | `opt-level=s` | accepted | Merge only if faster or smaller without smoke regression. |
| 10 | `codegen-units=32` | rejected | Merge only in local profile if app codegen improves. |
| 11 | `codegen-units=64` | rejected | Merge only if materially better than 32. |
| 12 | Release incremental | accepted | Merge only for local profile. |
| 13 | Disable local strip | rejected | Merge only if measurable or useful for diagnostics. |
| 14 | `cargo check` first-class fast loop | accepted | Merge doc/script improvements if they reduce edit-loop time. |
| 15 | ARM64 Docker cross image | accepted | Merge opt-in path only; keep standard `cross` as default until repeated timings justify switching. |
| 16 | Prebuilt amd64 cross image | rejected | Merge only if cold/first-run time becomes more predictable. |
| 17 | Persistent Docker Cargo cache audit | rejected | Merge only concrete cache fixes. |
| 18 | Container-local sccache trial | accepted | Merge only as an explicit opt-in check-loop experiment; never make host sccache required. |
| 19 | sccache cache dir on `/private/tmp` | rejected | Merge only if experiment 18 works and cache path is robust. |
| 20 | sccache after crate split | accepted | Merge only if hit rate improves after accepted splits. |
| 21 | LLD linker in cross image | rejected | Merge only if link time improves and binary checks pass. |
| 22 | Mold linker in cross image | rejected | Merge only if faster than default/LLD and device smoke passes. |
| 23 | cargo-chef-style dependency prewarm | rejected | Merge only if cold rebuild improves. |
| 24 | Split `mister-magik-core` crate | rejected | Merge only if rebuild boundaries improve. |
| 25 | Split `mister-magik-platform` crate | rejected | Merge only if UI edits stop rebuilding platform code. |
| 26 | Split `mister-magik-catalog` crate | accepted | Merge only if UI changes avoid catalog-heavy recompilation. |
| 27 | Slint generated UI crate | accepted | Merge only if non-Slint edits avoid Slint build/codegen. |
| 28 | Slint build fingerprint cache | accepted | Merge only if cache invalidates correctly. |
| 29 | Slint feature pruning on master | rejected | Merge only if Slint master remains easy to update/rebase. |
| 30 | Monomorphization/LLVM-lines pass | accepted | Merge only if app-bin codegen drops. |

## Experiment details

### 1. Baseline harness

- **Hypothesis:** Reliable wall-time and Cargo timing rows will make later
  experiments comparable.
- **Branch:** `codex/compile-exp-01-baseline-harness`
- **Changed files:** `scripts/bench-debug-build.sh`, `magik-gui/BUILD.md`, this
  report.
- **Commands:**
  - `bash -n scripts/bench-debug-build.sh`
  - `scripts/bench-debug-build.sh --scenario arm-check-launcher --state noop-warm --samples 1 --warmups 0 --label baseline-harness-smoke-v3`
  - `scripts/bench-debug-build.sh --scenario arm-check-launcher --state noop-warm --samples 5 --warmups 1 --label baseline-arm-check-launcher-warm`
  - `scripts/bench-debug-build.sh --scenario arm-check-launcher --state touch-rust-bin --samples 3 --warmups 1 --label baseline-arm-check-launcher-touch-rust-bin`
- **Results:**
  - Warm no-op ARM launcher check: median wall `2.904s`, median Cargo `1.18s`.
  - Touch `src/ui_runner.rs`: median wall `5.761s`, median Cargo `3.8s`,
    app-bin check unit `2.6s`.
  - The harness initially found an old `build/debug-build-bench.tsv` schema; it
    now rotates legacy TSVs before writing the current schema.
  - Very warm Cargo timing HTML can round total/unit time to `0.0s`; the harness
    now falls back to the log's `Finished ... in Ns` line for Cargo total time.
- **Decision:** accepted. Keep the harness and docs; continue adding experiment
  rows as branches are measured.

### 15. ARM64 Docker cross image

- **Hypothesis:** A native `linux/arm64` Docker image with an ARMv7 GNU
  cross-toolchain can avoid amd64 emulation on Apple Silicon and reduce build
  time.
- **Constraint:** Spend roughly two focused hours before declaring this failed.
- **Changed files:** `magik-gui/build-arm64-docker.sh`, `magik-gui/BUILD.md`,
  this report.
- **Required attempts before rejection:**
  - Ubuntu 20.04 arm64 base to preserve glibc 2.31 assumptions: passed.
  - Debian bookworm arm64 base if Ubuntu cross packages are incomplete: not
    needed; Ubuntu had the cross packages.
  - Minimal throwaway ARMv7 Rust crate before `magik-gui`: passed.
  - `cargo check --lib --no-default-features` before full Slint UI: passed.
  - Plain Docker invocation if `cross` rejects image metadata: required and
    passed.
- **Likely blockers to document:** armhf package availability, Rust host/target
  mismatch, cross image metadata, bindgen/libclang args, pkg-config/sysroot,
  bundled SQLite C build, Slint build dependencies, ARMv7 linker selection.
- **Acceptance:** `cross` or the equivalent Docker path compiles ARMv7 from
  Apple Silicon without amd64 emulation; ELF/shared-library checks pass; launcher
  smoke works on the MiSTer.
- **Commands:**
  - `docker build --platform linux/arm64 -f magik-gui/Dockerfile.cross-armv7 -t mister-magik-cross-armv7:ubuntu20-arm64 magik-gui`
  - `rustup toolchain add stable-aarch64-unknown-linux-gnu --profile minimal --force-non-host`
  - `rustup target add armv7-unknown-linux-gnueabihf --toolchain stable-aarch64-unknown-linux-gnu`
  - `DOCKER_DEFAULT_PLATFORM=linux/arm64 RUSTC_WRAPPER= cross check --lib --target armv7-unknown-linux-gnueabihf --no-default-features --locked`
  - Native Docker smoke crate build with mounted
    `stable-aarch64-unknown-linux-gnu` toolchain.
  - Native Docker `cargo check --lib --target armv7-unknown-linux-gnueabihf --no-default-features --locked`.
  - Native Docker `cargo check --target armv7-unknown-linux-gnueabihf --features ui --locked` with `MISTER_UI_BUILD_SCOPE=launcher`.
  - Native Docker `cargo build --target armv7-unknown-linux-gnueabihf --profile release-opts --features ui --locked` with `MISTER_UI_BUILD_SCOPE=launcher`.
  - `scripts/mister put /private/tmp/mister-arm64-magik-target/armv7-unknown-linux-gnueabihf/release-opts/mister-magik-fb /media/fat/mister-magik/mister-magik-fb`
  - `scripts/run-rust.sh launcher 5`
- **Results:**
  - Docker server is `linux/arm64`; the checked-in Ubuntu 20.04 ARMv7 cross
    image builds and runs as `aarch64`.
  - Inside the image, `arm-linux-gnueabihf-gcc -dumpmachine` reports
    `arm-linux-gnueabihf`; GCC 9.4.0, clang 10, and libclang are available.
  - `cross` 0.2.5 fails with `Dynamic loader not found:
    /lib64/ld-linux-x86-64.so.2` when forced to `linux/arm64`. Reading the
    installed cross source shows the reason: on non-Linux hosts it rewrites the
    sysroot to `x86_64-unknown-linux-gnu`, so an arm64 container receives an
    x86_64 Linux Rust toolchain.
  - Mounting `stable-aarch64-unknown-linux-gnu` directly into the same arm64
    Docker image works. The throwaway crate produced an ARMv7 ELF with
    interpreter `/lib/ld-linux-armhf.so.3`.
  - `magik-gui` library check passed in `6.76s`.
  - Launcher-scoped Slint UI check passed in `37.64s`.
  - Launcher-scoped `release-opts` binary built in `43.11s`; binary size
    `4,447,972` bytes.
  - ELF check: 32-bit ARM hard-float executable with interpreter
    `/lib/ld-linux-armhf.so.3`.
  - Dynamic dependencies: `libgcc_s.so.1`, `libpthread.so.0`, `libm.so.6`,
    `libdl.so.2`, `libc.so.6`, and `ld-linux-armhf.so.3`.
  - MiSTer launcher smoke passed with the exact arm64-Docker-built binary:
    steady `57-61fps`, final `242 frames in 5.0s = 48.3 fps avg`.
- **Implementation:**
  - Added `magik-gui/build-arm64-docker.sh` as an opt-in plain-Docker path.
  - The script builds the `linux/arm64` image, mounts the Linux/aarch64 Rust
    toolchain, clears `RUSTC_WRAPPER`, applies Cortex-A9/NEON Rust flags, uses
    `/private/tmp/mister-magik-arm64-target` by default, and supports the local
    non-video build profiles.
  - `--video` is intentionally rejected for now; the FFmpeg path remains on
    the standard `build-arm.sh` / `cross` route until separately measured.
- **Decision:** accepted as an opt-in experiment path. Do not replace
  `build-arm.sh` yet; first run repeated timing samples against the warmed
  standard `cross` image and decide whether this becomes default.

### 16. Prebuilt amd64 cross image

- **Hypothesis:** Replacing `Cross.toml`'s `dockerfile = "Dockerfile.cross-armv7"`
  with a named, prebuilt amd64 image can remove per-run Dockerfile resolution
  overhead and make no-op/warm builds more predictable.
- **Changed files:** temporary `magik-gui/Cross.toml` edit only; reverted after
  measurement.
- **Commands:**
  - `docker build --platform linux/amd64 -f magik-gui/Dockerfile.cross-armv7 -t mister-magik-cross-armv7:ubuntu20-amd64 magik-gui`
  - Temporary `Cross.toml`: `image = "mister-magik-cross-armv7:ubuntu20-amd64"`.
  - `scripts/bench-debug-build.sh --scenario build-ui-opts --state noop-warm --samples 3 --warmups 1 --label exp16-cross-dockerfile-noop`
  - `scripts/bench-debug-build.sh --scenario build-ui-opts --state noop-warm --samples 3 --warmups 1 --label exp16-cross-prebuilt-image-noop`
  - `scripts/bench-debug-build.sh --scenario build-ui-opts --state touch-rust-bin --samples 3 --warmups 1 --label exp16-cross-prebuilt-image-touch-rust`
  - `scripts/bench-debug-build.sh --scenario build-ui-opts --state touch-rust-bin --samples 3 --warmups 1 --label exp16-cross-dockerfile-touch-rust`
- **Results:**
  - Cached amd64 image build completed in about `1.16s`.
  - Dockerfile-config no-op samples: wall `3.793s`, `2.628s`, `2.653s`;
    median wall `2.653s`; median Cargo about `1.09s`.
  - Prebuilt-image no-op samples: wall `1.874s`, `1.876s`, `1.840s`; median
    wall `1.874s`; median Cargo about `0.95s`.
  - Prebuilt-image Rust-edit samples: wall `22.922s`, `985.082s`, `22.978s`;
    median wall `22.978s`; median Cargo about `21.40s`. The `985s` outlier had
    normal Cargo time and appears to be Docker/OrbStack wall-time stall rather
    than Rust work.
  - Dockerfile-config Rust-edit samples: wall `23.311s`, `23.834s`, `23.691s`;
    median wall `23.691s`; median Cargo about `21.53s`.
- **Interpretation:**
  - The named image saves roughly `0.7-0.8s` on warm/no-op and Rust-edit runs.
  - The win is real but small, and `Cross.toml` would become less portable
    because every machine would need the local image tag to exist.
  - It did not make wall time more predictable; the biggest observed outlier
    happened on the prebuilt-image path.
- **Decision:** rejected for mainline. Keep `Cross.toml` on the portable
  Dockerfile path and rely on Docker layer cache for the image. Revisit only if
  we publish a versioned image or repeated cold-start runs show a larger win.

### 2. Local launcher scope by default

- **Hypothesis:** Most local fast deploys do not need every benchmark scene, so
  using `MISTER_UI_BUILD_SCOPE=launcher` for local `--fast` builds will reduce
  optimized build time and binary size.
- **Branch:** `codex/compile-exp-02-launcher-scope-fast`
- **Changed files:** `magik-gui/build-arm.sh`, `scripts/deploy-rust.sh`,
  `magik-gui/BUILD.md`, `scripts/bench-debug-build.sh`, this report.
- **Commands:**
  - `scripts/bench-debug-build.sh --scenario build-ui-fast --state noop-warm --samples 1 --warmups 0 --label exp02-current-fast-all-scope`
  - `scripts/bench-debug-build.sh --scenario build-ui-fast-launcher --state noop-warm --samples 1 --warmups 0 --label exp02-fast-launcher-scope`
  - `scripts/bench-debug-build.sh --scenario build-ui-fast --state touch-rust-bin --samples 3 --warmups 0 --label exp02-current-fast-all-scope-touch-rust-bin`
  - `scripts/bench-debug-build.sh --scenario build-ui-fast-launcher --state touch-rust-bin --samples 3 --warmups 0 --label exp02-fast-launcher-scope-touch-rust-bin`
  - `magik-gui/build-arm.sh --fast`
  - `magik-gui/build-arm.sh --fast --all-scenes`
  - `magik-gui/scripts/check-arm-shared-libs.sh magik-gui/target/armv7-unknown-linux-gnueabihf/release/mister-magik-fb`
  - `scripts/deploy-rust.sh --fast`
  - `scripts/run-rust.sh launcher 5`
- **Initial results:**
  - Current default all-scope `build-arm.sh --fast`: wall `108.118s`, Cargo log
    `1m 44s`, binary `6,302,348` bytes.
  - Launcher-scoped `MISTER_UI_BUILD_SCOPE=launcher build-arm.sh --fast`: wall
    `89.156s`, Cargo log `1m 25s`, binary `5,357,180` bytes.
  - Single-sample delta: about `18.96s` faster wall time and `945,168` bytes
    smaller.
- **Repeat edit-loop results:**
  - Current all-scope `--fast` after touching `src/ui_runner.rs`: wall samples
    `95.750s`, `70.494s`, `68.273s`; median wall `70.494s`; median Cargo `67s`;
    binary `6,302,348` bytes.
  - Launcher-scoped `--fast` after touching `src/ui_runner.rs`: wall samples
    `83.025s`, `64.378s`, `62.523s`; median wall `64.378s`; median Cargo `61s`;
    binary `5,357,180` bytes.
  - Median edit-loop delta: `6.116s` wall and `6s` Cargo faster, plus `945,168`
    bytes smaller.
- **Implementation:**
  - `build-arm.sh --fast` now defaults to `MISTER_UI_BUILD_SCOPE=launcher`.
  - `build-arm.sh --fast --all-scenes` preserves the old full-scene behavior.
  - `build-arm.sh --ui-scope launcher|arcade|all` allows explicit measurement
    and local overrides.
  - `release-device`, profiling, and `--video` default to all-scenes; `--video`
    rejects non-`all` scopes because `video_playback.slint` is a bench scene.
  - `deploy-rust.sh --fast` inherits the launcher-scoped fast path, with
    `--all-scenes` and `--ui-scope` forwarded to `build-arm.sh`.
- **Verification:**
  - `magik-gui/build-arm.sh --fast` reported `ui_scope=launcher` and built
    `5,357,180` byte release binary.
  - `magik-gui/build-arm.sh --fast --all-scenes` reported `ui_scope=all` and
    built `6,302,348` byte release binary.
  - Shared-library check passed with expected glibc/libgcc/pthread/m/dl loader
    dependencies and no dynamic FFmpeg libraries.
  - `scripts/deploy-rust.sh --fast` deployed the `5,357,180` byte binary to the
    MiSTer.
  - `scripts/run-rust.sh launcher 5` started the launcher on-device, routed the
    960x540 framebuffer, opened three pads, reached steady `57-61fps` after
    startup, and exited with `done: 248 frames in 5.0s = 49.5 fps avg`.
- **Decision:** accepted. Merge the scoped fast-build default and keep
  `--all-scenes` as the explicit full UI path.

### 3. Bench scenes feature split

- **Hypothesis:** Benchmark Slint scenes should not be part of ordinary UI
  builds; isolating them behind a feature will keep normal launcher/arcade
  builds smaller and reduce accidental generated-code invalidation.
- **Branch:** `codex/compile-exp-03-bench-scenes-feature`
- **Changed files:** `magik-gui/Cargo.toml`, `magik-gui/build.rs`,
  `magik-gui/src/ui_runner.rs`, `magik-gui/build-arm.sh`, `scripts/dev-rust`,
  `scripts/bench-toolchain.sh`, `magik-gui/BUILD.md`, this report.
- **Implementation:**
  - Added Cargo feature `bench-scenes`.
  - `video` now implies `bench-scenes`.
  - `build.rs` only compiles `ui/bench/*.slint` when `bench-scenes` is enabled.
  - Rust generated-module imports, scene list entries, effect-bench Slint
    overlay code, console-scroll helpers, and video scene helpers are gated on
    `mister_bench_scenes`.
  - `build-arm.sh --all-scenes` adds `bench-scenes`.
  - `scripts/dev-rust check-arm-ui-full` and `build-arm-debug-full` use
    `--features ui,bench-scenes`.
  - `scripts/bench-toolchain.sh` passes `--all-scenes` by default so toolchain
    benchmarks keep compiling and running the benchmark scenes.
- **Verification:**
  - `scripts/dev-rust check-arm-ui` passed without `bench-scenes`.
  - `scripts/dev-rust check-arm-ui-full` passed with `bench-scenes`.
  - `magik-gui/build-arm.sh --fast` passed with `features: ui`, `ui_scope=launcher`,
    binary `5,357,180` bytes.
  - `magik-gui/build-arm.sh --fast --all-scenes` passed with
    `features: ui,bench-scenes`, `ui_scope=all`, binary `6,302,348` bytes.
- **Decision:** accepted. Merge the feature split; benchmark scenes are now
  explicit build opt-ins.

### 4. Video feature isolation

- **Hypothesis:** Normal launcher/arcade UI builds should not compile FFmpeg or
  video playback code; video work should remain opt-in.
- **Branch:** `codex/compile-exp-04-video-feature-isolation`
- **Changed files:** `magik-gui/Cargo.toml`, `magik-gui/build.rs`,
  `magik-gui/src/ui_runner.rs`, `magik-gui/build-arm.sh`, `magik-gui/BUILD.md`,
  this report.
- **Implementation:**
  - Existing optional dependency `ffmpeg-the-third` remains behind feature
    `video`.
  - Feature `video` now implies `bench-scenes`, because `video_playback.slint`
    is part of the benchmark scene set.
  - `build-arm.sh --video` still opts into video; non-video `--fast` and
    launcher checks use only feature `ui`.
  - `build-arm.sh` rejects `--video` with non-`all` UI scope so the generated
    video scene cannot be missing.
- **Verification:**
  - `cargo tree --target armv7-unknown-linux-gnueabihf --features ui --no-default-features -e features`
    produced no `ffmpeg*`, `avcodec`, or `avformat` entries.
  - `cargo tree --target armv7-unknown-linux-gnueabihf --features ui,video --no-default-features -e features`
    showed `ffmpeg-the-third`, `ffmpeg-sys-the-third`, `avcodec`, `avformat`,
    `swscale`, and `static` feature entries.
  - `scripts/dev-rust check-arm-ui` passed with normal launcher UI scope and no
    video feature.
- **Decision:** accepted. Normal UI builds avoid video crates/codegen; video
  remains explicit and all-scene scoped.

### 5. Add `release-fast-dev` profile

- **Hypothesis:** A local-only optimized profile with no LTO, more codegen
  units, incremental compilation, and no strip will greatly reduce Rust edit-loop
  time while leaving `release-device` as the production profile.
- **Branch:** `codex/compile-exp-05-release-fast-dev`
- **Changed files:** `magik-gui/Cargo.toml`, `magik-gui/build-arm.sh`,
  `scripts/deploy-rust.sh`, `scripts/bench-debug-build.sh`,
  `magik-gui/BUILD.md`, this report.
- **Implementation:**
  - Added `[profile.release-fast-dev]` inheriting from `release`.
  - Disabled LTO, enabled incremental compilation, set `codegen-units = 64`,
    and disabled strip.
  - Added `build-arm.sh --fast-dev` and `deploy-rust.sh --fast-dev`.
  - Added `build-ui-fast-dev` to the local benchmark harness.
- **Commands:**
  - `magik-gui/build-arm.sh --fast-dev`
  - `scripts/bench-debug-build.sh --scenario build-ui-fast-dev --state touch-rust-bin --samples 3 --warmups 1 --label exp05-fast-dev-touch-rust-bin`
  - `magik-gui/scripts/check-arm-shared-libs.sh magik-gui/target/armv7-unknown-linux-gnueabihf/release-fast-dev/mister-magik-fb`
- **Results:**
  - First cold `--fast-dev` build succeeded in about `3m21s`; binary size
    `11,224,828` bytes.
  - Touch `src/ui_runner.rs` `--fast-dev` samples: wall `9.988s`, `9.662s`,
    `9.874s`; median wall `9.874s`; median Cargo `7.80s`; binary
    `11,224,828` bytes.
  - Compared with accepted launcher-scoped `release` touch-Rust median
    `64.378s` wall / `61s` Cargo, this is about `54.5s` faster wall time for
    the common Rust edit loop.
  - Shared-library check passed with expected glibc/libgcc/pthread/m/dl loader
    dependencies and no dynamic FFmpeg libraries.
- **Pending verification:**
  - Re-run longer visual/performance smoke if `--fast-dev` becomes the default
    daily deploy profile.
- **Device smoke:**
  - `scripts/deploy-rust.sh --fast-dev` deployed the `11,224,828` byte binary.
  - `scripts/run-rust.sh launcher 5` started the launcher on-device, routed the
    960x540 framebuffer, opened three pads, settled at `56-61fps` after catalog
    startup, and exited with `done: 231 frames in 5.0s = 46.1 fps avg`.
- **Decision:** accepted. Keep `release-fast-dev` as an explicit local edit-loop
  profile; do not make it the production profile.

### 6. No-LTO fast build

- **Hypothesis:** Disabling LTO alone may capture most of the edit-loop win
  without also enabling incremental compilation, increasing codegen units, or
  disabling strip.
- **Branch:** `codex/compile-exp-06-no-lto-only`
- **Changed files:** temporary only; the `release-no-lto` Cargo profile,
  `build-arm.sh --no-lto`, and `build-ui-no-lto` harness scenario were removed
  after measurement because this was not a winner.
- **Command:**
  - `scripts/bench-debug-build.sh --scenario build-ui-no-lto --state touch-rust-bin --samples 3 --warmups 1 --label exp06-no-lto-touch-rust-bin`
- **Result:**
  - The warmup did not reach the first measured sample after about 36 minutes.
  - The log showed a fresh optimized profile recompiling the Slint dependency
    graph (`i-slint-common`, `i-slint-compiler`, `resvg`, `image`, etc.).
  - The run was aborted and the orphaned Docker process was terminated.
- **Interpretation:**
  - No-LTO alone is not a useful standalone local profile in this project.
  - The accepted `release-fast-dev` result appears to depend on the combination
    of no LTO plus incremental reuse and higher codegen parallelism, not merely
    on the LTO setting.
- **Decision:** rejected. Do not keep a `release-no-lto` profile.

### 7. Thin-LTO fast build

- **Hypothesis:** The daily `release` profile was documented as thin LTO, but
  Cargo's `lto = true` actually means fat LTO. Switching local `release` to
  `lto = "thin"` should preserve an optimized binary while cutting edit-loop
  link time. Cargo docs: <https://doc.rust-lang.org/cargo/reference/profiles.html#lto>.
- **Branch:** `codex/compile-exp-07-real-thin-lto`
- **Changed files:** `magik-gui/Cargo.toml`, `magik-gui/BUILD.md`,
  `magik-gui/build-arm.sh`, this report.
- **Implementation:**
  - Changed `[profile.release] lto = true` to `lto = "thin"`.
  - Left `[profile.release-device] lto = "fat"` and `codegen-units = 1`, so the
    production size/profile path remains fat LTO.
  - Kept `release-fast-dev` as the fastest explicit edit-loop profile.
- **Commands:**
  - `scripts/bench-debug-build.sh --scenario build-ui-fast --state touch-rust-bin --samples 3 --warmups 1 --label exp07-thin-lto-touch-rust-bin`
  - `magik-gui/scripts/check-arm-shared-libs.sh magik-gui/target/armv7-unknown-linux-gnueabihf/release/mister-magik-fb`
  - `scripts/deploy-rust.sh --fast`
  - `scripts/run-rust.sh launcher 5`
  - `scripts/mister run "tail -n 80 /tmp/mister-magik-launcher.log; pidof mister-magik-fb 2>/dev/null || true"`
- **Results:**
  - Thin-LTO `release` touch `src/ui_runner.rs` samples: wall `25.611s`,
    `24.781s`, `24.991s`; median wall `24.991s`; median Cargo `22.98s`; binary
    `6,004,364` bytes.
  - Compared with the accepted launcher-scoped pre-change `release` median
    `64.378s` wall / `61s` Cargo / `5,357,180` bytes, real thin LTO is about
    `39.4s` faster but `647,184` bytes larger.
  - Compared with accepted `release-fast-dev`, thin LTO is slower for Rust edits
    (`24.991s` vs `9.874s`) but produces a much smaller optimized binary
    (`6,004,364` vs `11,224,828` bytes).
  - Shared-library check passed with expected glibc/libgcc/pthread/m/dl loader
    dependencies and no dynamic FFmpeg libraries.
- **Device smoke:**
  - `scripts/deploy-rust.sh --fast` deployed the `6,004,364` byte binary.
  - `scripts/run-rust.sh launcher 5` started the launcher on-device, routed the
    960x540 framebuffer, opened three pads, settled at `57-61fps` after catalog
    startup, and exited with `done: 247 frames in 5.0s = 49.2 fps avg`.
- **Decision:** accepted. Use real thin LTO for the normal local `release` /
  `--fast` profile; use `release-fast-dev` when the priority is shortest Rust
  edit-loop time.

### 8. `opt-level=2`

- **Hypothesis:** `opt-level = 2` may reduce optimized binary size or compile
  work for local deploys while preserving launcher frame pacing on the MiSTer.
- **Branch:** `codex/compile-exp-08-opt-level-2`
- **Changed files:** `magik-gui/Cargo.toml`, `magik-gui/build-arm.sh`,
  `scripts/bench-debug-build.sh`, `scripts/deploy-rust.sh`,
  `magik-gui/BUILD.md`, this report.
- **Implementation:**
  - Added `[profile.release-opt2]` inheriting from `release` with
    `opt-level = 2`.
  - Added `build-arm.sh --opt2`, `deploy-rust.sh --opt2`, and
    `build-ui-opt2` in the benchmark harness.
  - Kept the default `release` profile at `opt-level = 3`; `release-opt2` is an
    explicit local profile.
- **Commands:**
  - `scripts/bench-debug-build.sh --scenario build-ui-opt2 --state touch-rust-bin --samples 3 --warmups 1 --label exp08-opt2-touch-rust-bin`
  - `magik-gui/scripts/check-arm-shared-libs.sh magik-gui/target/armv7-unknown-linux-gnueabihf/release-opt2/mister-magik-fb`
  - `scripts/deploy-rust.sh --opt2`
  - `scripts/run-rust.sh launcher 5`
  - `scripts/mister run "tail -n 80 /tmp/mister-magik-launcher.log; pidof mister-magik-fb 2>/dev/null || true"`
- **Results:**
  - Touch `src/ui_runner.rs` `release-opt2` samples: wall `25.487s`,
    `24.759s`, `25.267s`; median wall `25.267s`; median Cargo `22.92s`;
    binary `5,451,492` bytes.
  - Compared with accepted thin-LTO `release`, `release-opt2` is essentially
    compile-time neutral (`25.267s` vs `24.991s` median wall) but
    `552,872` bytes smaller (`5,451,492` vs `6,004,364`).
  - Shared-library check passed with expected glibc/libgcc/pthread/m/dl loader
    dependencies and no dynamic FFmpeg libraries.
- **Device smoke:**
  - `scripts/deploy-rust.sh --opt2` deployed the `5,451,492` byte binary.
  - `scripts/run-rust.sh launcher 5` started the launcher on-device, routed the
    960x540 framebuffer, opened three pads, settled at `57-61fps` after catalog
    startup, and exited with `done: 245 frames in 5.0s = 49.0 fps avg`.
- **Decision:** accepted as an explicit optional local profile. It is smaller
  than `release` with comparable compile time, but `release-fast-dev` remains
  the fastest Rust edit loop and `release-device` remains the production path.

### 9. `opt-level=s`

- **Hypothesis:** `opt-level = "s"` may reduce the local deploy binary
  substantially and possibly improve optimized edit-loop time while keeping
  launcher frame pacing acceptable.
- **Branch:** `codex/compile-exp-09-opt-level-s`
- **Changed files:** `magik-gui/Cargo.toml`, `magik-gui/build-arm.sh`,
  `scripts/bench-debug-build.sh`, `scripts/deploy-rust.sh`,
  `magik-gui/BUILD.md`, this report.
- **Implementation:**
  - Added `[profile.release-opts]` inheriting from `release` with
    `opt-level = "s"`.
  - Added `build-arm.sh --opts`, `deploy-rust.sh --opts`, and
    `build-ui-opts` in the benchmark harness.
  - Kept the default `release` profile at `opt-level = 3`; `release-opts` is an
    explicit local profile.
- **Commands:**
  - `scripts/bench-debug-build.sh --scenario build-ui-opts --state touch-rust-bin --samples 3 --warmups 1 --label exp09-opts-touch-rust-bin`
  - `magik-gui/scripts/check-arm-shared-libs.sh magik-gui/target/armv7-unknown-linux-gnueabihf/release-opts/mister-magik-fb`
  - `scripts/deploy-rust.sh --opts`
  - `scripts/run-rust.sh launcher 5`
  - `scripts/mister run "tail -n 80 /tmp/mister-magik-launcher.log; pidof mister-magik-fb 2>/dev/null || true"`
- **Results:**
  - Touch `src/ui_runner.rs` `release-opts` samples: wall `25.221s`,
    `22.821s`, `23.710s`; median wall `23.710s`; median Cargo `21.74s`;
    binary `4,447,972` bytes.
  - Compared with accepted thin-LTO `release`, `release-opts` is about `1.28s`
    faster on median wall time and `1,556,392` bytes smaller (`4,447,972` vs
    `6,004,364`).
  - Compared with accepted `release-opt2`, `release-opts` is about `1.56s`
    faster on median wall time and `1,003,520` bytes smaller (`4,447,972` vs
    `5,451,492`).
  - Shared-library check passed with expected glibc/libgcc/pthread/m/dl loader
    dependencies and no dynamic FFmpeg libraries.
- **Device smoke:**
  - `scripts/deploy-rust.sh --opts` deployed the `4,447,972` byte binary.
  - `scripts/run-rust.sh launcher 5` started the launcher on-device, routed the
    960x540 framebuffer, opened three pads, settled at `56-61fps` after catalog
    startup, and exited with `done: 225 frames in 5.0s = 45.0 fps avg`.
- **Decision:** accepted as the smallest local optimized smoke profile. It is
  not the fastest edit-loop profile (`release-fast-dev` remains faster), and it
  does not replace `release-device`, but it is useful when local deploy/copy
  size matters.

### 10. `codegen-units=32`

- **Hypothesis:** Raising `release` from Cargo's default non-incremental
  `codegen-units = 16` to `32` may improve final app codegen parallelism for
  Rust edit loops.
- **Branch:** `codex/compile-exp-10-cgu32`
- **Changed files:** temporary only; the `release-cgu32` Cargo profile,
  `build-arm.sh --cgu32`, and `build-ui-cgu32` harness scenario were removed
  after measurement because this was not a winner.
- **Command:**
  - `scripts/bench-debug-build.sh --scenario build-ui-cgu32 --state touch-rust-bin --samples 3 --warmups 1 --label exp10-cgu32-touch-rust-bin`
- **Results:**
  - Touch `src/ui_runner.rs` `release-cgu32` samples: wall `32.068s`,
    `32.086s`, `38.712s`; median wall `32.086s`; median Cargo `29.27s`;
    binary `6,016,652` bytes.
  - Compared with accepted thin-LTO `release`, CGU32 is about `7.1s` slower on
    median wall time and `12,288` bytes larger (`6,016,652` vs `6,004,364`).
- **Decision:** rejected. Do not keep a CGU32 local profile.

### 11. `codegen-units=64`

- **Hypothesis:** Raising `release` to `codegen-units = 64` may be materially
  better than CGU32 for app-bin codegen parallelism.
- **Branch:** `codex/compile-exp-11-cgu64`
- **Changed files:** temporary only; the `release-cgu64` Cargo profile,
  `build-arm.sh --cgu64`, and `build-ui-cgu64` harness scenario were removed
  after measurement because this was not a winner.
- **Command:**
  - `scripts/bench-debug-build.sh --scenario build-ui-cgu64 --state touch-rust-bin --samples 3 --warmups 1 --label exp11-cgu64-touch-rust-bin`
- **Results:**
  - Touch `src/ui_runner.rs` `release-cgu64` samples: wall `38.286s`,
    `36.526s`, `36.111s`; median wall `36.526s`; median Cargo `34.10s`;
    binary `6,041,228` bytes.
  - Compared with CGU32, CGU64 is about `4.4s` slower on median wall time and
    `24,576` bytes larger.
  - Compared with accepted thin-LTO `release`, CGU64 is about `11.5s` slower on
    median wall time and `36,864` bytes larger.
- **Decision:** rejected. CGU64 is materially worse than both CGU32 and the
  existing `release` profile; do not keep the profile.

### 12. Release incremental

- **Hypothesis:** Enabling incremental compilation on top of the accepted
  thin-LTO `release` profile may capture a meaningful part of the
  `release-fast-dev` win while keeping LTO, strip, default CGUs, and a smaller
  deployable binary.
- **Branch:** `codex/compile-exp-12-release-incremental`
- **Changed files:** `magik-gui/Cargo.toml`, `magik-gui/build-arm.sh`,
  `scripts/bench-debug-build.sh`, `scripts/deploy-rust.sh`,
  `magik-gui/BUILD.md`, this report.
- **Implementation:**
  - Added `[profile.release-incr]` inheriting from `release` with
    `incremental = true`.
  - Added `build-arm.sh --incr`, `deploy-rust.sh --incr`, and
    `build-ui-incr` in the benchmark harness.
  - Kept default `release` non-incremental; `release-incr` is an explicit local
    profile.
- **Commands:**
  - `scripts/bench-debug-build.sh --scenario build-ui-incr --state touch-rust-bin --samples 3 --warmups 1 --label exp12-incr-touch-rust-bin`
  - `magik-gui/scripts/check-arm-shared-libs.sh magik-gui/target/armv7-unknown-linux-gnueabihf/release-incr/mister-magik-fb`
  - `scripts/deploy-rust.sh --incr`
  - `scripts/run-rust.sh launcher 5`
  - `scripts/mister run "tail -n 80 /tmp/mister-magik-launcher.log; pidof mister-magik-fb 2>/dev/null || true"`
- **Results:**
  - Touch `src/ui_runner.rs` `release-incr` samples: wall `18.413s`,
    `17.468s`, `19.395s`; median wall `18.413s`; median Cargo `15.82s`;
    binary `5,824,140` bytes.
  - Compared with accepted thin-LTO `release`, `release-incr` is about `6.58s`
    faster on median wall time and `180,224` bytes smaller (`5,824,140` vs
    `6,004,364`).
  - Compared with `release-fast-dev`, `release-incr` is slower (`18.413s` vs
    `9.874s`) but much smaller (`5,824,140` vs `11,224,828` bytes).
  - Shared-library check passed with expected glibc/libgcc/pthread/m/dl loader
    dependencies and no dynamic FFmpeg libraries.
- **Device smoke:**
  - `scripts/deploy-rust.sh --incr` deployed the `5,824,140` byte binary.
  - `scripts/run-rust.sh launcher 5` started the launcher on-device, routed the
    960x540 framebuffer, opened three pads, settled at `56-61fps` after catalog
    startup, and exited with `done: 231 frames in 5.0s = 46.1 fps avg`.
- **Decision:** accepted as an explicit local optimized profile. Keep normal
  `release` non-incremental for the clean daily profile, use `release-incr`
  when optimized edit-loop speed matters, and use `release-fast-dev` when raw
  Rust edit-loop speed matters most.

### 13. Disable local strip

- **Hypothesis:** Disabling strip may reduce local edit-loop time enough to be
  useful for diagnostics or iteration.
- **Branch:** `codex/compile-exp-13-disable-strip`
- **Changed files:** temporary only; the `release-nostrip` Cargo profile,
  `build-arm.sh --nostrip`, and `build-ui-nostrip` harness scenario were
  removed after measurement because this was not a winner.
- **Command:**
  - `scripts/bench-debug-build.sh --scenario build-ui-nostrip --state touch-rust-bin --samples 3 --warmups 1 --label exp13-nostrip-touch-rust-bin`
- **Results:**
  - Touch `src/ui_runner.rs` `release-nostrip` samples: wall `32.371s`,
    `34.561s`, `33.041s`; median wall `33.041s`; median Cargo `30.90s`;
    binary `10,489,492` bytes.
  - Compared with accepted thin-LTO `release`, no-strip is about `8.05s` slower
    and `4,485,128` bytes larger.
  - Compared with accepted `release-fast-dev`, no-strip is slower and only
    modestly smaller, without the broader no-LTO/incremental benefits.
- **Decision:** rejected. Do not keep a no-strip local profile; use
  `release-device-profile` for intentional symbolized profiling builds.

### 14. `cargo check` first-class fast loop

- **Hypothesis:** The fastest feedback path may already be `cross check`; making
  the right check command obvious and documenting measured edit-loop anchors can
  save local iteration time without changing compiler profiles.
- **Branch:** `codex/compile-exp-14-check-loop`
- **Changed files:** `scripts/dev-rust`, `magik-gui/BUILD.md`, this report.
- **Implementation:**
  - Added `scripts/dev-rust check-ui` as an alias for `check-arm-ui`.
  - Added `scripts/dev-rust check-ui-full` as an alias for
    `check-arm-ui-full`.
  - Added a recommended local loop ladder to `magik-gui/BUILD.md`: host library
    check, ARM library check, ARM UI check, full UI check, then deploy builds
    only when a device binary is needed.
- **Commands:**
  - `scripts/bench-debug-build.sh --scenario arm-check-launcher --state touch-slint-launcher --samples 3 --warmups 1 --label exp14-check-launcher-touch-slint-launcher`
  - `scripts/bench-debug-build.sh --scenario arm-check-launcher --state touch-slint-shared --samples 3 --warmups 1 --label exp14-check-launcher-touch-slint-shared`
- **Results:**
  - Existing baseline warm no-op `check-arm-ui`: median wall `2.904s`, median
    Cargo `1.18s`.
  - Existing Rust UI edit `check-arm-ui`: median wall `5.761s`, median Cargo
    `3.8s`.
  - Touch `ui/launcher.slint`: wall `26.985s`, `27.225s`, `27.502s`; median
    wall `27.225s`; median Cargo `25.1s`; top units were build script
    `~19.4s`, bin check `~4.0s`, lib check `~0.6s`.
  - Touch shared `ui/mister_window.slint`: wall `27.092s`, `27.089s`,
    `26.603s`; median wall `27.089s`; median Cargo `25.0s`; top units were
    build script `~19.3s`, bin check `~4.0s`, lib check `~0.6s`.
- **Interpretation:**
  - `check-ui` is already far faster than optimized deploy builds for Rust UI
    edits.
  - Slint edits are dominated by `build.rs` / Slint codegen; future Slint cache
    experiments should target build-script invalidation and generated UI reuse,
    not Rust profile settings.
- **Decision:** accepted. Keep the aliases and documented local loop ladder.

### 17. Persistent Docker Cargo cache audit

- **Hypothesis:** Local rebuild time may be losing cache reuse across Docker /
  cross invocations, especially if sccache or Cargo config leaks from the host.
- **Branch:** `codex/compile-exp-17-cache-audit`
- **Changed files:** none for this experiment; existing cache/sccache guards
  were inspected.
- **Commands / inspection:**
  - `rg -n "cross|Dockerfile|CARGO_HOME|target|docker|DOCKER_DEFAULT_PLATFORM|cache|sccache|RUSTC_WRAPPER" -S .cargo magik-gui scripts`
  - `sed -n '1,120p' .cargo/config.toml`
  - `sed -n '1,120p' magik-gui/.cargo/config.toml`
  - `sed -n '1,120p' /Users/nigelb/.cargo/config.toml`
  - `sed -n '1,120p' magik-gui/Cross.toml`
  - `sed -n '1,120p' magik-gui/Dockerfile.cross-armv7`
  - `orbctl status`, `orbctl doctor`, `docker ps`
- **Findings:**
  - Repo root `.cargo/config.toml` and `magik-gui/.cargo/config.toml` both set
    `[build] rustc-wrapper = ""`, protecting direct and repo-root Cargo
    invocations from a global sccache wrapper.
  - Current `/Users/nigelb/.cargo/config.toml` contains only macOS linker
    rustflags and no `rustc-wrapper`, so the earlier "sccache keeps being picked
    up" symptom is already guarded locally and no longer present globally.
  - `build-arm.sh`, `scripts/dev-rust`, `scripts/bench-toolchain.sh`, and
    `scripts/mister` explicitly export `RUSTC_WRAPPER=""`.
  - `cross` mounts the project target directory and Cargo home into the
    container; measured warm rebuilds confirm target artifacts and registry/git
    cache are already persistent across ordinary runs.
  - A pathological aborted Docker run left `docker info` / `docker ps` hanging;
    clean `orbctl stop` followed by `orbctl start` recovered Docker. This is an
    operational hazard, not a cache-miss fix.
- **Decision:** rejected / no-op. No cache configuration change is justified
  right now. Keep the existing `rustc-wrapper = ""` guards and explicit
  `RUSTC_WRAPPER=""` exports.

### 18. Container-local sccache trial

- **Hypothesis:** A compiler cache inside the cross container can speed local
  ARMv7 `check` / build loops without requiring host sccache or reintroducing
  the confusing global `RUSTC_WRAPPER` behavior.
- **Branch:** `codex/compile-exp-18-sccache`
- **Changed files:** `magik-gui/Dockerfile.cross-armv7-sccache`,
  `magik-gui/Cross.sccache.toml`, `scripts/bench-debug-build.sh`, this report.
- **Implementation:**
  - Built an opt-in `linux/amd64` cross image tagged
    `mister-magik-cross-armv7:sccache-amd64`.
  - Installed Mozilla `sccache 0.15.0` from the current GitHub release asset in
    that image. Ubuntu 20.04 apt did not provide an `sccache` package.
  - Added `Cross.sccache.toml` so the wrapper is target/container-local:
    `[target.armv7-unknown-linux-gnueabihf] rustc-wrapper = "sccache"`.
  - Added separate benchmark scenarios:
    `arm-check-launcher-sccache` and `build-ui-opts-sccache`.
  - Avoided global `RUSTC_WRAPPER=sccache`. A first attempt with global
    `RUSTC_WRAPPER` failed before Docker with:
    `could not execute process sccache ... rustc -vV`, because Cargo metadata
    ran on macOS and the host sccache had been uninstalled.
- **Commands:**
  - `docker build --platform linux/amd64 -f magik-gui/Dockerfile.cross-armv7-sccache -t mister-magik-cross-armv7:sccache-amd64 magik-gui`
  - `docker run --rm --platform linux/amd64 mister-magik-cross-armv7:sccache-amd64 sccache --version`
  - `scripts/bench-debug-build.sh --scenario arm-check-launcher-sccache --state noop-warm --samples 3 --warmups 1 --label exp18-sccache-check-noop`
  - `scripts/bench-debug-build.sh --scenario arm-check-launcher-sccache --state touch-rust-bin --samples 3 --warmups 1 --label exp18-sccache-check-touch-rust-bin`
  - `scripts/bench-debug-build.sh --scenario build-ui-opts-sccache --state touch-rust-bin --samples 3 --warmups 1 --label exp18-sccache-build-opts-touch-rust-bin`
- **Results:**
  - Image sanity: `sccache 0.15.0`.
  - No-op launcher check: wall `2.927s`, `2.227s`, `2.246s`; median `2.246s`.
  - Rust edit launcher check: wall `3.090s`, `3.034s`, `3.078s`; median
    `3.078s`. Recent non-sccache launcher-scope check anchor after experiment
    30 was `3.748s` median.
  - Rust edit `release-opts`: wall `24.782s`, `23.817s`, `24.801s`; median
    `24.782s`. Recent non-sccache `release-opts` anchor after experiment 30 was
    `24.348s` median. Binary size unchanged at `3,997,412` bytes.
- **Interpretation:**
  - sccache is useful for the fast `check-ui` loop after the crate/UI split,
    saving roughly `0.67s` on the measured Rust edit check path.
  - It does not materially improve the optimized `release-opts` build path.
  - The important correctness fix is isolation: if sccache is configured as a
    global `RUSTC_WRAPPER`, Cargo metadata on macOS still tries to run it even
    when the user has uninstalled host sccache. The repo default must continue
    clearing inherited wrappers, and sccache must remain opt-in.
- **Decision:** accepted as an opt-in experiment path for `check` loops only.
  Do not make it the default local build path and do not require host sccache.

### 19. sccache cache dir on `/private/tmp`

- **Hypothesis:** Moving the sccache cache to `/private/tmp` could avoid Docker
  bind-mount or project-target overhead.
- **Branch:** `codex/compile-exp-19-sccache-cache-dir`
- **Changed files:** none beyond the opt-in sccache harness from experiment 18.
- **Findings:**
  - The accepted sccache path uses `SCCACHE_DIR=/target/sccache` inside the
    Cross container config, but Cross did not expose an obvious
    `magik-gui/target/sccache` host directory after the run.
  - Mounting `/private/tmp` robustly would require extra Cross volume plumbing
    and would make the cache path more host-specific.
  - Because experiment 18 only improved the check loop and did not improve
    `release-opts`, a cache-dir relocation is not worth merging without
    stronger evidence.
- **Decision:** rejected. Keep sccache isolated and simple.

### 20. sccache after crate split

- **Hypothesis:** sccache becomes more useful after the catalog split,
  generated-UI crate split, Slint fingerprint cache, and launcher-scope pruning
  reduce the amount of project code that changes per edit.
- **Branch:** `codex/compile-exp-20-sccache-after-splits`
- **Changed files:** same opt-in harness/config as experiment 18.
- **Results:** The post-split measurement is the experiment 18 measurement:
  launcher-scope Rust edit check median improved from the experiment 30
  non-sccache anchor `3.748s` to `3.078s` with opt-in sccache.
- **Decision:** accepted as supporting evidence for the opt-in check-loop path.
  It does not change the decision for optimized builds.

### 21. LLD linker in cross image

- **Hypothesis:** Replacing GNU ld with LLD for the final ARMv7 link can reduce
  optimized Rust edit-loop time without changing Rust codegen.
- **Changed files:** temporary `magik-gui/Dockerfile.cross-armv7-lld` and
  temporary `magik-gui/Cross.toml` image/env edits; both reverted after
  measurement.
- **Commands / setup:**
  - Built `mister-magik-cross-armv7:ubuntu20-amd64-lld` from the existing
    Ubuntu 20.04 image plus package `lld`.
  - Direct GCC test: `arm-linux-gnueabihf-gcc -fuse-ld=lld` failed with
    `collect2: fatal error: cannot find 'ld'`.
  - Direct clang test succeeded:
    `clang --target=arm-linux-gnueabihf --sysroot=/usr/arm-linux-gnueabihf -fuse-ld=lld`.
  - Temporary `Cross.toml` pointed at the LLD image and passed through
    `CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER`.
  - Rust build env:
    `CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=clang` and
    `RUSTFLAGS='-C link-arg=--target=arm-linux-gnueabihf -C link-arg=--sysroot=/usr/arm-linux-gnueabihf -C link-arg=-fuse-ld=lld'`.
  - `scripts/bench-debug-build.sh --scenario build-ui-opts --state touch-rust-bin --samples 3 --warmups 1 --label exp21-lld-touch-rust`
  - `magik-gui/scripts/check-arm-shared-libs.sh magik-gui/target/armv7-unknown-linux-gnueabihf/release-opts/mister-magik-fb`
- **Results:**
  - Cold LLD image build took about `71s` and pulled an extra LLVM/LLD package
    set. This is a one-time image cost, not the edit-loop metric.
  - First Rust build with new linker flags succeeded; binary size was
    `4,446,804` bytes, about `1.14 KiB` smaller than GNU ld.
  - LLD Rust-edit samples: wall `22.615s`, `21.660s`, `1005.913s`; median wall
    `22.615s`; median Cargo `20.20s`.
  - Shared-library check passed with the expected glibc/libgcc/pthread/m/dl
    loader dependencies.
  - Compared with experiment 16's prebuilt GNU ld image path, LLD saves only
    about `0.36s` median wall time and about `1.2s` Cargo time. Compared with
    the default Dockerfile path, most of the observed win is explained by the
    prebuilt-image effect rather than LLD itself.
  - The third sample had another large Docker/OrbStack wall-time stall while
    Cargo itself reported a normal `20.12s`, so this path did not improve
    predictability.
- **Decision:** rejected for mainline. The clang+LLD route is technically
  viable, but the measured win is too small for the added image, env, and
  linker-argument complexity. Revisit only if link time becomes dominant after
  crate/codegen reductions.

### 22. Mold linker in cross image

- **Hypothesis:** `mold` might beat GNU ld and LLD for the final ARMv7 link,
  enough to justify a custom local cross image.
- **Changed files:** temporary `magik-gui/Dockerfile.cross-armv7-mold`,
  temporary `magik-gui/Cross.mold.toml`, and a temporary
  `build-ui-opts-mold` benchmark scenario. All were removed after measurement.
- **Research / setup:**
  - GitHub latest release lookup on 2026-06-10 found `mold 2.41.0`, published
    2026-04-13, with `mold-2.41.0-x86_64-linux.tar.gz` and SHA256
    `a3696680d99e692970590a178bc3a33d78d60d1c6dc9db7a11b557b02b751f5d`.
  - Built `mister-magik-cross-armv7:mold-amd64` from the existing Ubuntu 20.04
    cross image plus `clang`, `curl`, and the verified mold release tarball.
  - Image sanity: `mold 2.41.0`.
- **Commands / probes:**
  - `docker build --platform linux/amd64 -f magik-gui/Dockerfile.cross-armv7-mold -t mister-magik-cross-armv7:mold-amd64 magik-gui`
  - `docker run --rm --platform linux/amd64 mister-magik-cross-armv7:mold-amd64 mold --version`
  - First linker attempt used
    `-C link-arg=-fuse-ld=mold`; clang 10 rejected it with
    `invalid linker name in argument '-fuse-ld=mold'`.
  - Second linker attempt used
    `-C link-arg=--sysroot=/usr/arm-linux-gnueabihf` and
    `-C link-arg=-fuse-ld=/usr/local/bin/mold`; mold double-prefixed the sysroot
    and failed on
    `/usr/arm-linux-gnueabihf/usr/arm-linux-gnueabihf/lib/libc.so.6`.
  - Final linker attempt removed the explicit sysroot and used:
    `CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=clang` with
    `RUSTFLAGS='-C target-cpu=cortex-a9 -C target-feature=+neon -C link-arg=--target=arm-linux-gnueabihf -C link-arg=-fuse-ld=/usr/local/bin/mold'`.
  - `scripts/bench-debug-build.sh --scenario build-ui-opts-mold --state touch-rust-bin --samples 1 --warmups 0 --label exp22-mold-build-opts-touch-rust-bin-nosysroot`
  - `magik-gui/scripts/check-arm-shared-libs.sh magik-gui/target/armv7-unknown-linux-gnueabihf/release-opts/mister-magik-fb`
- **Results:**
  - Successful no-sysroot sample: wall `70.250s`, Cargo `68.1s`, binary
    `3,996,752` bytes.
  - Shared-library check passed with expected glibc/libgcc/pthread/m/dl loader
    dependencies.
  - The binary was about `660` bytes smaller than the GNU ld / sccache
    `release-opts` anchor (`3,997,412` bytes), but the edit-loop was far slower
    than the normal `24.348s` median and slower than LLD's `22.615s` median.
- **Decision:** rejected. Mold is technically viable only with a more fragile
  clang invocation and is not a local compile-time win for this project.

### 23. cargo-chef-style dependency prewarm

- **Hypothesis:** Prebuilding dependency layers with a `cargo-chef`-style Docker
  image could improve cold local cross builds.
- **Branch:** `codex/compile-exp-23-cargo-chef`
- **Changed files:** none.
- **Analysis:**
  - This is strong prior art for CI, where Docker image layers persist and the
    source checkout is copied into the image.
  - The local Cross workflow mounts the project target directory as `/target`
    and Cargo home as `/cargo`; those bind mounts are exactly what make normal
    local rebuilds persistent.
  - A dependency build baked into a Docker image layer would be hidden by the
    `/target` bind mount during Cross runs unless the workflow stopped using the
    normal Cross target mount.
  - Replacing Cross with a custom direct-Docker workflow would duplicate
    toolchain logic already covered by experiment 15's opt-in ARM64 Docker path
    and would not help the normal edit loop.
  - Cold-cost observations from experiments 18, 22, and 29 reinforce this:
    custom image work costs tens of seconds to minutes, while the daily wins
    came from Rust profile choice, scope pruning, crate boundaries, and Slint
    codegen caching.
- **Decision:** rejected for local Mac Cross builds. Keep this as CI-oriented
  prior art, not a local workflow change.

### 27. Slint generated UI crate

- **Hypothesis:** Moving Slint codegen and generated Rust modules into a
  dedicated path crate will let non-Slint edits to `mister-magik-fb` reuse the
  generated UI crate instead of rerunning/checking embedded generated modules in
  the app package.
- **Changed files:** `magik-gui/Cargo.toml`, `magik-gui/Cargo.lock`,
  `magik-gui/build.rs`, `magik-gui/src/ui_runner.rs`,
  `magik-gui/ui-generated/{Cargo.toml,build.rs,src/lib.rs}`, this report.
- **Implementation:**
  - Added `mister-magik-ui`, a path crate that owns `slint-build`, compiles the
    Slint roots, and exports generated modules (`launcher`, `controller`,
    `arcade_page`, `app`, plus bench/video modules behind features).
  - The main crate now depends on `mister-magik-ui` under feature `ui` and uses
    it as the `slint_ui` namespace from `ui_runner.rs`.
  - The main crate's `build.rs` now only emits local cfgs for
    `MISTER_UI_BUILD_SCOPE` and `bench-scenes`; Slint compilation moved out.
  - `bench-scenes` and `video` propagate into the UI crate so full-scene builds
    still compile the benchmark Slint roots.
- **Commands:**
  - `cargo check --target armv7-unknown-linux-gnueabihf --features ui --manifest-path magik-gui/Cargo.toml`
    to update `Cargo.lock` for the new local path package; the direct host run
    then failed at `libsqlite3-sys` because it was intentionally outside the
    cross container and lacked `arm-linux-gnueabihf-gcc`.
  - `scripts/dev-rust check-ui`
  - `scripts/bench-debug-build.sh --scenario arm-check-launcher --state touch-rust-bin --samples 3 --warmups 1 --label exp27-ui-crate-touch-rust-bin`
  - `scripts/bench-debug-build.sh --scenario arm-check-launcher --state touch-slint-launcher --samples 3 --warmups 1 --label exp27-ui-crate-touch-slint-launcher`
  - `scripts/bench-debug-build.sh --scenario build-ui-opts --state touch-rust-bin --samples 3 --warmups 1 --label exp27-ui-crate-build-opts-touch-rust-bin`
  - `scripts/dev-rust fmt`
  - `scripts/dev-rust check-ui`
  - `scripts/dev-rust check-ui-full`
  - `magik-gui/scripts/check-arm-shared-libs.sh magik-gui/target/armv7-unknown-linux-gnueabihf/release-opts/mister-magik-fb`
- **Results:**
  - Launcher ARM check after touching `src/ui_runner.rs`: wall samples
    `3.593s`, `3.682s`, `3.542s`; median wall `3.593s`; median Cargo `2.0s`;
    app-bin check unit `0.7-0.8s`.
  - Previous post-catalog anchor for a Rust UI edit was about `6.569s` check
    wall before the rejected core split, and the earlier baseline was `5.761s`.
    This split is a real improvement for non-Slint UI runner edits.
  - Launcher ARM check after touching `ui/launcher.slint`: wall samples
    `32.805s`, `34.610s`, `32.802s`; median wall `32.805s`; median Cargo
    `30.0s`. Top units are now explicit: `mister-magik-ui` build script
    `21-22s`, `mister-magik-ui` check `5.0-5.4s`, app-bin check `1.1s`.
  - `release-opts` build after touching `src/ui_runner.rs`: wall samples
    `26.718s`, `23.441s`, `25.485s`; median wall `25.485s`; median Cargo
    `22.87s`; binary `4,435,684` bytes.
  - Compared with experiment 26's post-catalog `release-opts` anchor
    (`39.455s` after catalog edits, binary `4,452,068`) and experiment 9's
    Rust-edit `release-opts` anchor (`23.710s`, binary `4,447,972`), optimized
    local builds remain in the same range while the check loop improves
    materially.
  - `scripts/dev-rust check-ui` and `scripts/dev-rust check-ui-full` both pass
    with the new crate. The parallel verification run had Cargo lock waits, but
    both commands completed successfully.
  - Shared-library check passed with expected dependencies only:
    `libgcc_s.so.1`, `libpthread.so.0`, `libm.so.6`, `libdl.so.2`, `libc.so.6`,
    and `ld-linux-armhf.so.3`.
- **Tradeoff:**
  - Slint edits still pay Slint codegen; this split does not solve experiment 28.
  - The generated UI code is now visible as a separate timing unit, which makes
    future Slint-cache and monomorphization work easier to reason about.
- **Decision:** accepted. Keep the generated UI path crate because it gives a
  clear non-Slint UI edit-loop win, preserves full-scene/video feature paths,
  and makes future Slint-specific caching experiments better isolated.

### 30. Monomorphization/LLVM-lines pass

- **Hypothesis:** After the generated UI crate split, the remaining app-bin
  codegen cost is likely concentrated in a few large UI/runtime functions. Use
  `cargo-llvm-lines` to identify obvious launcher-scope codegen that can be
  removed from local builds without weakening the full build path.
- **Changed files:** `magik-gui/build.rs`,
  `magik-gui/ui-generated/{build.rs,src/lib.rs}`, `magik-gui/src/{fb.rs,main.rs,ui_runner.rs}`,
  `magik-gui/BUILD.md`, this report.
- **Tooling:**
  - Installed `cargo-llvm-lines v0.4.46` as a local analysis tool; this is not a
    project dependency.
  - Direct host ARM run failed because macOS lacks `arm-linux-gnueabihf-gcc` for
    bundled SQLite.
  - Host fallback also failed because Slint master's macOS/AppKit path currently
    hits no-`std` `std::vec::Vec` references under this feature set.
  - Successful path used the accepted experiment 15 ARM64 Docker image with the
    Linux/aarch64 Rust toolchain and ARMv7 target, installing the Linux
    `cargo-llvm-lines` binary into `/private/tmp/mister-magik-arm64-target/cargo-tools`.
- **LLVM-lines command:**
  - `docker run --rm --platform linux/arm64 ... /target/cargo-tools/bin/cargo-llvm-lines llvm-lines --bin mister-magik-fb --target armv7-unknown-linux-gnueabihf --features ui --locked -- --cfg mister_ui_scope_launcher`
- **LLVM-lines findings:**
  - Total launcher-scope ARMv7 IR before the prune: `143,210` lines across
    `3,651` copies.
  - Largest app-owned entries included `ui_runner::run_launcher_loop`
    (`2,971` lines), `ui_runner::run_arcade_page_loop` (`1,265` lines),
    `ui_runner::run_ui` (`893` lines), `input::sniff` (`891` lines),
    `ui_runner::run_blend_velocity_loop` (`793` lines), and
    `ui_runner::run_frame_loop` (`567` lines).
  - Interpretation: local launcher scope was still compiling standalone
    `arcade_page` and `demo`/`app` generated modules and dispatch code even
    though normal fast local builds exercise the launcher/controller path.
- **Implementation:**
  - Made `mister_ui_scope_launcher` mean literal `MISTER_UI_BUILD_SCOPE=launcher`
    rather than treating `arcade` as launcher scope.
  - In `mister-magik-ui`, launcher scope now compiles only
    `controller_test.slint` and `launcher.slint`; `app.slint` and
    `arcade_page.slint` remain compiled for `arcade` and `all` scopes.
  - In `ui_runner.rs`, standalone `demo` and `arcade_page` scenes are excluded
    only under `mister_ui_scope_launcher`; `launcher`, `controller_test`, and
    the existing custom `blend_velocity` path remain available.
  - Added targeted dead-code allowances for helpers that are intentionally
    unused in narrower local scopes.
- **Commands:**
  - `scripts/dev-rust check-ui`
  - `scripts/dev-rust check-arm-arcade-ui`
  - `scripts/dev-rust check-ui-full`
  - `scripts/bench-debug-build.sh --scenario arm-check-launcher --state touch-rust-bin --samples 3 --warmups 1 --label exp30-launcher-prune-touch-rust-bin-check`
  - `scripts/bench-debug-build.sh --scenario build-ui-opts --state touch-rust-bin --samples 3 --warmups 1 --label exp30-launcher-prune-build-opts-touch-rust-bin`
  - `magik-gui/scripts/check-arm-shared-libs.sh magik-gui/target/armv7-unknown-linux-gnueabihf/release-opts/mister-magik-fb`
- **Results:**
  - Launcher ARM check after touching `src/ui_runner.rs`: wall `5.432s`,
    `3.660s`, `3.748s`; median wall `3.748s`; median Cargo `2.1s`; app-bin
    check unit `0.7-0.8s`. This is effectively neutral against experiment 27's
    `3.593s` median.
  - `release-opts` build after touching `src/ui_runner.rs`: wall `24.348s`,
    `23.947s`, `25.772s`; median wall `24.348s`; median Cargo `21.01s`.
    This is about `1.14s` faster than experiment 27's `25.485s` post-UI-crate
    median.
  - Binary size dropped from `4,435,684` bytes to `3,997,412` bytes for
    launcher-scope `release-opts` (`-438,272` bytes).
  - `check-ui`, `check-arm-arcade-ui`, and `check-ui-full` all pass, proving the
    launcher prune does not remove the arcade/all-scene build paths.
  - Shared-library check passed with expected dependencies only:
    `libgcc_s.so.1`, `libpthread.so.0`, `libm.so.6`, `libdl.so.2`, `libc.so.6`,
    and `ld-linux-armhf.so.3`.
- **Decision:** accepted. Keep the launcher-scope prune because it reduces local
  optimized codegen and binary size while preserving `arcade` and `all` scopes.
  Further codegen reductions should target `run_launcher_loop` itself; broad
  platform splitting is not justified by experiment 25's measurements.

### 28. Slint build fingerprint cache

- **Hypothesis:** Slint edit time is dominated by build-script codegen; avoiding
  unnecessary top-level Slint roots in launcher-scope builds may reduce the
  `build.rs` run before we attempt a more invasive generated-code cache.
- **Attempted sub-experiment:** Temporarily compile `ui/app.slint` / `demo` only
  when `bench-scenes` is enabled, while keeping launcher, controller, and arcade
  roots in the normal launcher scope.
- **Changed files:** temporary `magik-gui/build.rs` and
  `magik-gui/src/ui_runner.rs` edits; reverted after measurement.
- **Commands:**
  - `scripts/dev-rust check-ui`
  - `scripts/dev-rust check-ui-full`
  - `scripts/bench-debug-build.sh --scenario arm-check-launcher --state touch-slint-launcher --samples 3 --warmups 1 --label exp28-no-demo-launcher-touch-slint-launcher`
- **Results:**
  - `check-ui` and `check-ui-full` both passed with the temporary change.
  - Launcher Slint edit samples after touching `ui/launcher.slint`: wall
    `867.598s`, `457.381s`, `32.166s`; median wall is contaminated by
    Docker/OrbStack stalls.
  - Cargo times were `22.8s`, `30.5s`, `29.1s`; build-script units were
    `17.6s`, `22.0s`, `21.8s`. This does not improve on the previous
    experiment 14 anchor of about `27.2s` wall / `25.1s` Cargo / `19.4s`
    build-script time.
  - The temporary change also removed the `demo` scene from ordinary fast builds,
    which is a behavior change for little or no compile-time benefit.
- **Decision:** rejected for this sub-experiment. Do not gate `app.slint` /
  `demo` out of launcher-scope builds. A real Slint cache experiment should
  target generated output reuse or build-script dependency fingerprints, not
  just this one top-level root.

- **Accepted sub-experiment:** Add a content fingerprint to the new
  `mister-magik-ui` build script. The build script declares the selected
  Slint/font/svg inputs with `cargo:rerun-if-changed`, hashes their bytes plus
  `SLINT_FONT_SIZES`, feature flags, and the build script itself, and skips
  `slint-build` only when the fingerprint matches and every expected generated
  `.rs` file already exists in `OUT_DIR`.
- **Changed files:** `magik-gui/ui-generated/build.rs`,
  `magik-gui/BUILD.md`, this report.
- **Commands:**
  - `scripts/dev-rust check-ui`
  - `scripts/bench-debug-build.sh --scenario arm-check-launcher --state touch-slint-launcher --samples 3 --warmups 1 --label exp28-slint-fingerprint-touch-slint-launcher`
  - Temporary content probe: add a harmless comment to `ui/launcher.slint`.
  - `scripts/bench-debug-build.sh --scenario arm-check-launcher --state noop-warm --samples 1 --warmups 0 --label exp28-slint-fingerprint-content-change`
  - Remove the temporary `ui/launcher.slint` comment.
  - `scripts/dev-rust fmt`
  - `scripts/dev-rust check-ui`
  - `scripts/dev-rust check-ui-full`
- **Results:**
  - Mtime-only `ui/launcher.slint` touch with unchanged content: wall samples
    `7.761s`, `7.261s`, `7.463s`; median wall `7.463s`; median Cargo `5.5s`.
    The `mister-magik-ui` build-script unit dropped to `0.1s`, with no Slint
    codegen in the top units.
  - Before this cache, experiment 27 measured the same touch state at median
    wall `32.805s`, median Cargo `30.0s`, and `mister-magik-ui` build-script
    time `21-22s`.
  - Real source-content invalidation probe: a temporary comment in
    `ui/launcher.slint` produced wall `33.253s`, Cargo `30.1s`, and
    `mister-magik-ui` build-script `21.6s`, proving the cache does not reuse
    stale generated code when bytes change.
  - Restoring the source after the temporary comment triggered one full
    regeneration, as expected, and `scripts/dev-rust check-ui` passed.
  - `scripts/dev-rust check-ui-full` passed, proving the cache key handles the
    `bench-scenes` feature/input set separately.
- **Tradeoff:**
  - This does not make real Slint edits faster; it only avoids expensive
    regeneration for unchanged-content saves, mtime churn, and harness touches.
  - The simple fingerprint lives in `OUT_DIR`, so it is target/profile scoped
    and does not introduce a cross-profile shared generated-code cache.
- **Decision:** accepted. Keep the fingerprint guard because it is safe,
  invalidates on real content/config changes, and cuts noisy mtime-only Slint
  rebuilds from roughly half a minute to under eight seconds.

### 29. Slint feature pruning on master

- **Hypothesis:** Since we intentionally stay on Slint git `master`, perhaps
  more default-adjacent Slint features can be disabled without pinning Slint to
  a crates.io release.
- **Branch:** `codex/compile-exp-29-slint-feature-prune`
- **Changed files:** temporary `magik-gui/Cargo.toml` and
  `magik-gui/ui-generated/Cargo.toml` edits; reverted after the probe.
- **Current feature audit:**
  - `cargo tree -e features --target armv7-unknown-linux-gnueabihf --features ui -i slint --locked`
  - Active Slint features are already minimal for this no-std-style MiSTer
    build: `compat-1-2`, `renderer-software`, `unsafe-single-threaded`, and
    `libm`.
  - Default Slint features remain disabled; `std` remains disabled to avoid
    system font/fontconfig assumptions on MiSTer.
- **Probe:**
  - Removed `libm` from the `slint` dependency in both the main crate and the
    generated UI crate.
  - Ran `scripts/dev-rust check-ui`.
- **Result:**
  - The ARM UI check failed in Slint master's `i-slint-core`.
  - Root cause: Slint imports `num_traits::Float` and uses float methods such
    as `powi`, `powf`, `sin`, `cos`, `sqrt`, `ceil`, `floor`, and `round`.
    `num_traits::Float` is gated behind `std` or `libm`, so with both disabled
    current Slint master does not compile.
  - The run produced 52 Slint core errors before aborting.
- **Decision:** rejected / no-op. Keep `libm`; it is the cost of staying
  no-std-ish while avoiding Slint's `std` feature. Future Slint speedups should
  come from input-scope pruning, generated-code caching, or upstream Slint
  improvements, not from removing the remaining public Slint features.

### 24-26. Crate split pre-audit

- **Hypothesis:** Splitting stable, pure-ish modules out of the `mister-magik-fb`
  package can reduce the amount of app-bin code rechecked/recodegenerated after
  UI edits. The split only helps if the new crates break real dependency edges;
  moving files without reducing edges is churn.
- **Commands / inspection:**
  - `find magik-gui/src -maxdepth 1 -type f -name '*.rs' -print | sort`
  - `wc -l magik-gui/src/*.rs | sort -n`
  - `rg -n "use crate::|mister_magik_fb::" magik-gui/src/{arcade_catalog,controller_db,launcher,library_bench,preview_worker,runtime_status,setup_nav,input_info,input_repeat,framebuffer_copy,effects}.rs`
  - `rg -n "slint::|crate::fb|crate::fpga|libc::|/dev/|std::os::|mmap|ioctl|linux|evdev|ffmpeg|rusqlite|quick_xml|walkdir|zune_png|swash" magik-gui/src/*.rs`
- **Findings:**
  - `ui_runner.rs` is the dominant file at about `5,937` lines; splitting crates
    will not help Slint edits unless it reduces what `ui_runner` directly
    instantiates or imports.
  - `library_bench.rs` (`~3,036` lines), `arcade_catalog.rs` (`~940` lines),
    `controller_db.rs` (`~589` lines), `launcher.rs` (`~910` lines), and
    `setup_nav.rs` (`~451` lines) are plausible split targets.
  - The current library target is already host-testable and excludes framebuffer,
    FPGA, Linux input, and Slint. That means a new crate must improve binary
    rebuild boundaries, not merely make tests nicer.
  - `arcade_catalog`, `library_bench`, and `preview_worker` form the cleanest
    catalog/media cluster. They depend on filesystem/XML/SQLite/PNG-ish crates
    but not Slint or framebuffer. This is the strongest candidate for experiment
    26.
  - `launcher.rs` and `setup_nav.rs` are not pure yet: they import
    `crate::input::PadState` / `PadPool` and contain MiSTer process/fifo actions.
    Before experiment 24 can split `mister-magik-core`, input snapshots and
    launch side effects need a boundary.
  - `controller_db.rs` already depends on `input_info::PadInfo`, which is a
    better portable type than the Linux input module. This can move with a core
    or controller crate after the setup navigation boundary is cleaned up.
  - Platform modules (`fb.rs`, `fpga.rs`, `input.rs`, `mr_audio.rs`, `vt.rs`,
    `display_config.rs`) clearly belong together if experiment 25 proceeds, but
    moving them first is unlikely to help the common UI edit loop because
    `ui_runner` still calls them directly.
- **Proposed order:**
  1. Experiment 26 first: create a catalog/media crate containing
     `arcade_catalog`, `library_bench`, and `preview_worker`; measure
     `touch-rust-bin`, `touch-rust-lib`, and `touch-slint-launcher` before/after.
  2. Experiment 24 second: extract portable nav/input snapshots from `input.rs`
     so `launcher`, `setup_nav`, `input_repeat`, `input_info`, and
     `controller_db` can live in a core/controller crate without Linux device
     dependencies.
  3. Experiment 25 third: move framebuffer/FPGA/input/audio/vt/display config
     into a platform crate only if experiments 24/26 show binary rebuild
     boundaries improve.
- **Acceptance update:** each split must include before/after harness rows for
  `arm-check-launcher`, `build-ui-opts`, and at least one Slint-touch state. A
  split is rejected if it only moves files and the `mister-magik-fb` bin unit
  remains unchanged or slower.
- **Experiment 25 follow-up:**
  - Re-audited after experiments 26-28. A real platform crate is not just
    `fb`/`fpga`/`vt`/`mr_audio`: `fb.rs`, `fpga.rs`, and `vt.rs` call
    `boot_analytics`, which calls `runtime_status`; `fb.rs` also uses
    `framebuffer_copy` and `vsync_pacer`; `display_config.rs` ties `fb`,
    `fpga`, and `ui_display` together; `ui_runner.rs` still directly owns the
    runtime orchestration and platform type usage. Moving only the obvious files
    would either create a circular dependency back into the main library or
    require moving pure/tested modules that experiment 24 already showed are not
    a good split target right now.
  - Added `touch-rust-platform` to `scripts/bench-debug-build.sh` so platform
    module touches are measurable without moving files.
  - Commands:
    - `bash -n scripts/bench-debug-build.sh`
    - `scripts/bench-debug-build.sh --scenario arm-check-launcher --state touch-rust-platform --samples 3 --warmups 1 --label exp25-preaudit-touch-rust-platform-check`
    - `scripts/bench-debug-build.sh --scenario build-ui-opts --state touch-rust-platform --samples 3 --warmups 1 --label exp25-preaudit-touch-rust-platform-build-opts`
  - Results:
    - `fb.rs` touch, launcher ARM check: wall `3.682s`, `4.103s`, `3.586s`;
      median wall `3.682s`; app-bin check unit `0.7-1.0s`.
    - `fb.rs` touch, `release-opts` build: wall `25.368s`, `26.835s`,
      `23.856s`; median wall `25.368s`; binary `4,435,684` bytes.
    - These are effectively the same as experiment 27's post-UI-crate
      `ui_runner.rs` touch anchors: check median `3.593s`; `release-opts`
      median `25.485s`.
  - Interpretation: after the UI generated crate split, the remaining app-bin
    check cost is already below one second and the optimized app-bin rebuild is
    not platform-specific enough to justify a broad platform/common crate
    extraction. Any honest platform crate would need a larger common-runtime
    split first, which duplicates the rejected direction of experiment 24.
- **Decision:** experiment 25 rejected for now. Keep the benchmark state because
  it is useful evidence, but do not move platform modules until a future
  `llvm-lines`/codegen pass identifies a specific platform hot spot worth
  isolating.

### 26. Split `mister-magik-catalog` crate

- **Hypothesis:** Moving catalog/media scanning out of the main package into a
  path dependency will reduce rebuild work after catalog edits and create a
  stable boundary for later UI/core/platform splits.
- **Changed files:** `magik-gui/catalog/Cargo.toml`,
  `magik-gui/catalog/src/lib.rs`, moved `arcade_catalog.rs`,
  `library_bench.rs`, and `preview_worker.rs` into `magik-gui/catalog/src/`,
  `magik-gui/Cargo.toml`, `magik-gui/src/lib.rs`, `magik-gui/src/main.rs`,
  `scripts/bench-debug-build.sh`, `Cargo.lock`, this report.
- **Implementation:**
  - Added path dependency `mister-magik-catalog = { path = "catalog" }`.
  - Re-exported `arcade_catalog`, `library_bench`, and `preview_worker` from
    `mister-magik-fb` so existing call sites can keep using the same module
    names.
  - Moved catalog dependencies (`quick-xml`, `walkdir`, `rusqlite`, catalog-side
    `libc`, etc.) into the new crate. Kept `swash`, `zune-png`, `serde`, and
    `serde_json` in the main crate where still used.
  - Added `touch-rust-catalog` to `scripts/bench-debug-build.sh`; after the
    split it touches `catalog/src/arcade_catalog.rs`.
- **Commands:**
  - Before split:
    `scripts/bench-debug-build.sh --scenario arm-check-launcher --state touch-rust-catalog --samples 3 --warmups 1 --label exp26-before-check-launcher-touch-catalog`
  - Before split:
    `scripts/bench-debug-build.sh --scenario build-ui-opts --state touch-rust-catalog --samples 3 --warmups 1 --label exp26-before-build-opts-touch-catalog`
  - After split:
    `scripts/dev-rust check`
  - After split:
    `scripts/dev-rust check-ui`
  - After split:
    `scripts/bench-debug-build.sh --scenario arm-check-launcher --state touch-rust-catalog --samples 3 --warmups 1 --label exp26-after-check-launcher-touch-catalog`
  - After split:
    `scripts/bench-debug-build.sh --scenario build-ui-opts --state touch-rust-catalog --samples 3 --warmups 1 --label exp26-after-build-opts-touch-catalog`
- **Results:**
  - Before split, catalog edit `check-ui` samples: wall `7.395s`, `7.309s`,
    `7.270s`; median wall `7.309s`; median Cargo `5.3s`; app bin check unit
    about `3.1s`.
  - After split, catalog edit `check-ui` samples: wall `7.112s`, `6.969s`,
    `6.931s`; median wall `6.969s`; median Cargo `5.2s`; app bin check unit
    about `2.8s`; new catalog crate check unit about `0.5-0.6s`.
  - Before split, catalog edit `release-opts` samples: wall `41.501s`,
    `41.152s`, `41.376s`; median wall `41.376s`; median Cargo about `37.98s`;
    binary `4,447,972` bytes.
  - After split, catalog edit `release-opts` samples: wall `38.784s`,
    `39.455s`, `39.858s`; median wall `39.455s`; median Cargo about `36.58s`;
    binary `4,452,068` bytes.
  - Delta: about `0.34s` faster on launcher check and about `1.92s` faster on
    optimized `release-opts` catalog edits, with a `4,096` byte binary increase.
- **Interpretation:**
  - This is not a huge compile-time win by itself, but it is a real targeted
    improvement and a clean architectural boundary for catalog/library work.
  - The remaining heavy cost is still the main Slint/UI binary. Experiments 24
    and 25 should focus on reducing `ui_runner`'s direct dependency surface,
    not merely moving more files.
- **Decision:** accepted. Keep the catalog/media crate split.

### 24. Split `mister-magik-core` crate

- **Hypothesis:** Launcher/controller/setup navigation should be separable from
  Linux device I/O so the pure state-machine code can move into a smaller core
  crate. This only helps compile time if the Slint binary stops owning all of
  the navigation/check logic directly.
- **Current status:** boundary prep kept; full crate move rejected.
- **Changed files so far:** `magik-gui/src/input_state.rs`,
  `magik-gui/src/input.rs`, `magik-gui/src/lib.rs`, `magik-gui/src/main.rs`,
  `magik-gui/src/launcher.rs`, `magik-gui/src/setup_nav.rs`, this report.
- **Implementation so far:**
  - Added portable `input_state` module containing `PadState`, `PadLayout`, and
    `layout_profile_name`.
  - `launcher.rs` now imports `PadState` from `input_state`, not the Linux
    joystick module.
  - `setup_nav.rs` now imports `PadState` / `PadInfo` /
    `layout_profile_name` from `input_state`.
  - Replaced the direct `setup_nav -> input::PadPool` dependency with a
    `SetupPadSource` trait. `PadPool` implements the trait in `input.rs`, but
    setup navigation no longer needs to know about Linux joystick polling.
  - `input.rs` keeps Linux js event decoding close to `PadReader` via a local
    extension trait implemented for the portable `PadState`.
- **Verification:**
  - `scripts/dev-rust check`
  - `scripts/dev-rust fmt`
  - `scripts/dev-rust test`
  - `scripts/dev-rust check-ui`
  - `scripts/dev-rust check-ui-full`
- **Measured crate-move attempt:**
  - Temporarily created `magik-gui/core` (`mister-magik-core`) and moved
    `controller_db`, `input_info`, `input_repeat`, `input_state`, and
    `setup_nav` into it.
  - Added `touch-rust-core` to `scripts/bench-debug-build.sh` and measured
    `input_state.rs` edits before and after the move.
  - Before crate move, `check-ui` samples: wall `6.569s`, `6.398s`, `6.742s`;
    median wall `6.569s`; median Cargo about `4.8s`; app bin check about
    `2.8s`.
  - After crate move, `check-ui` samples: wall `7.257s`, `7.120s`, `7.162s`;
    median wall `7.162s`; median Cargo about `5.3s`; app bin check about
    `3.0s`; core crate check about `0.6s`.
  - Before crate move, `release-opts` samples: wall `39.658s`, `37.851s`,
    `37.693s`; median wall `37.851s`; median Cargo about `34.94s`; binary
    `4,452,068` bytes.
  - After crate move, `release-opts` samples: wall `35.250s`, `38.508s`,
    `38.279s`; median wall `38.279s`; median Cargo about `35.30s`; binary
    `4,456,164` bytes.
  - The crate move was reverted after measurement because it failed the merge
    rule.
- **Interpretation:**
  - The useful boundary prep is the `input_state` extraction and
    `SetupPadSource` trait; they reduce Linux-input coupling without adding a
    Cargo package boundary.
  - Moving the portable modules into a separate crate adds package overhead while
    the Slint binary still depends on their public types and navigation state.
  - A future retry should first split `launcher.rs` side effects (`stop_mister`,
    fifo launch, reboot/reset actions) from pure navigation state and then
    measure again.
- **Decision:** rejected for the actual core crate split. Keep the smaller
  boundary-prep refactor because it improves architecture and enables a future
  lower-risk retry, but do not add `mister-magik-core` now.

## Results

- Initial baseline rows are in local `build/debug-build-bench.tsv`.
- Curated first baseline:
  - `arm-check-launcher`, `noop-warm`, 5 measured samples after 1 warmup:
    median wall `2.904s`; median Cargo `1.18s`.
  - `arm-check-launcher`, `touch-rust-bin`, 3 measured samples after 1 warmup:
    median wall `5.761s`; median Cargo `3.8s`; app-bin unit `2.6s`.
- Interpretation: the warm no-op loop is mostly Docker/cross process overhead,
  while Rust UI edits still concentrate time in the final `mister-magik-fb` bin
  check unit. This keeps crate splitting and app-bin monomorphization high-value
  experiments.
- Experiment 2 is accepted: launcher-scoped `--fast` improves Rust edit-loop
  median wall time by about 6 seconds, cuts the local fast binary by about
  0.9 MB, keeps full-scene builds available with `--all-scenes`, and passed the
  MiSTer launcher smoke.
- Experiment 3 is accepted: ordinary UI builds no longer compile generated
  benchmark Slint scenes, while `--all-scenes`, full dev checks, video builds,
  and `bench-toolchain.sh` opt into `bench-scenes`.
- Experiment 4 is accepted: non-video `ui` builds have no FFmpeg dependency tree,
  while `ui,video` builds pull the expected FFmpeg crates and all-scene Slint
  path.
- Experiment 5 is accepted: `release-fast-dev` cut the Rust edit-loop median
  from `64.378s` to `9.874s` wall time, passed shared-library checks, and passed
  a MiSTer launcher smoke. It remains an explicit local profile rather than the
  production profile.
- Experiment 6 is rejected: no-LTO-only failed to reach the first measured
  sample after about 36 minutes because the fresh profile rebuilt the Slint
  dependency graph. The temporary profile and harness hook were removed.
- Experiment 7 is accepted: Cargo `lto = true` was fat LTO, not thin LTO.
  Switching local `release` to `lto = "thin"` cut the Rust edit-loop median to
  `24.991s` while keeping device smoke clean. `release-device` remains fat LTO.
- Experiment 8 is accepted as an optional local profile: `release-opt2` keeps a
  comparable Rust edit-loop median (`25.267s`) to thin-LTO `release` while
  shrinking the launcher-scoped binary to `5,451,492` bytes and passing the
  MiSTer launcher smoke.
- Experiment 9 is accepted as the smallest local optimized smoke profile:
  `release-opts` reached `23.710s` median Rust edit-loop wall time, shrank the
  launcher-scoped binary to `4,447,972` bytes, and passed the MiSTer launcher
  smoke.
- Experiment 10 is rejected: CGU32 was slower and slightly larger than the
  accepted thin-LTO `release` profile.
- Experiment 11 is rejected: CGU64 was slower/larger than CGU32 and the
  accepted thin-LTO `release` profile.
- Experiment 12 is accepted: `release-incr` cut the optimized Rust edit-loop
  median to `18.413s`, kept the binary to `5,824,140` bytes, passed
  shared-library checks, and passed the MiSTer launcher smoke.
- Experiment 13 is rejected: disabling strip alone was slower and produced a
  `10,489,492` byte binary.
- Experiment 14 is accepted: `check-ui` / `check-ui-full` aliases and BUILD.md
  guidance make the fastest non-deploy feedback loop first-class. Slint edits
  still take about `27s`, mostly in build-script Slint codegen.
- Experiment 15 is accepted as an opt-in path: native `linux/arm64` Docker plus
  a Linux/aarch64 Rust toolchain produced and smoke-tested a valid ARMv7 MiSTer
  binary. `cross` itself cannot currently drive that image on macOS because it
  mounts an x86_64 Linux Rust sysroot.
- Experiment 16 is rejected: a named prebuilt amd64 image saves about
  `0.7-0.8s` in warm cases but makes `Cross.toml` depend on a local image tag
  and did not improve Docker/OrbStack wall-time predictability.
- Experiment 17 found no mergeable cache change: Cargo home, git/registry
  caches, target artifacts, and sccache opt-out are already persistent/guarded.
  It did uncover a Docker/OrbStack recovery note after an aborted benchmark.
- Experiment 18 is accepted as an opt-in sccache path for fast check loops:
  target/container-local sccache cut the launcher-scope Rust edit check median
  from `3.748s` to `3.078s`, while optimized `release-opts` stayed neutral.
  Host/global sccache remains deliberately disabled.
- Experiment 19 is rejected: moving the sccache cache to `/private/tmp` would
  add Cross volume complexity without evidence of a release-build win.
- Experiment 20 is accepted as post-split evidence for experiment 18: sccache is
  only worth keeping after the crate/UI split because the remaining check-loop
  work is small enough for cache overhead to matter.
- Experiment 21 is rejected: clang+LLD can link the ARMv7 binary and passes
  shared-library checks, but the measured win over GNU ld is too small for the
  added image/linker complexity.
- Experiment 22 is rejected: mold can link a valid ARMv7 binary only with a
  fragile no-sysroot clang setup, and the measured Rust edit `release-opts`
  sample was `70.250s`, much slower than GNU ld/LLD.
- Experiment 23 is rejected for local Cross builds: cargo-chef-style image
  prewarming is CI-shaped, while Cross bind-mounts `/target` and `/cargo` for
  exactly the caches the local loop needs.
- Experiment 24 is rejected for the full core crate move: extracting
  `mister-magik-core` made `input_state` edit checks slower (`6.569s` →
  `7.162s`) and slightly worsened `release-opts` median (`37.851s` →
  `38.279s`). The `input_state` / `SetupPadSource` boundary prep remains.
- Experiment 25 is rejected: a platform crate would require moving intertwined
  runtime/framebuffer/boot-analytics code and measured platform-file edits were
  already in the same range as ordinary UI touches.
- Experiment 26 is accepted: moving catalog/media scanning into
  `mister-magik-catalog` improved catalog edit rebuilds modestly
  (`check-ui` median `7.309s` → `6.969s`; `release-opts` median `41.376s` →
  `39.455s`) and gives later split work a cleaner boundary.
- Experiment 27 is accepted: moving Slint-generated modules into
  `mister-magik-ui` cut non-Slint UI runner check edits to about `3.593s`
  median and made Slint build/codegen a separate timing unit.
- Experiment 28 has one rejected sub-experiment: removing `app.slint` / `demo`
  from launcher-scope builds did not improve Slint edit time enough and changed
  the local fast-build scene surface. The real generated-code cache work remains
  open.
- Experiment 28's fingerprint cache is accepted: unchanged-content Slint mtime
  churn dropped from roughly `32.805s` to `7.463s`, while real content changes
  still force full Slint regeneration.
- Experiment 29 is rejected: current Slint master needs `libm` when `std` is
  disabled; removing it produced Slint core `num_traits::Float` / float-method
  compile failures.
- Experiment 30 is accepted: the LLVM-lines pass led to launcher-scope pruning,
  reducing the launcher `release-opts` binary from `4,435,684` to `3,997,412`
  bytes and improving the Rust edit optimized median to about `24.348s`.
