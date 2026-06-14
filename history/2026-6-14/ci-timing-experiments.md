# CI timing experiments

Goal: reduce GitHub Actions wall time without skipping the main builds, ARM
profiles, host checks, tests, artifact uploads, or shared-library checks.

## Final accepted state

Accepted changes:

- Cache host debug build outputs and broad ARM target/profile build outputs.
- Keep Cargo registry/git caches, the `cross` binary cache, and minimal FFmpeg
  cache.
- Move ARM CI from `ubuntu-22.04` to `ubuntu-24.04`.
- Upgrade checkout/cache/upload actions to Node 24-compatible major versions.
- Disable Cargo HTML timing reports in CI with `MISTER_CARGO_TIMINGS=0`.
- Keep the custom Ubuntu 20.04 cross image, but remove unused packages:
  `clang`, `curl`, `file`, native `g++`, ARM `g++`, `git`, and stale
  `CXX_armv7_unknown_linux_gnueabihf`.
- Add a GLIBC symbol-version guard to `check-arm-shared-libs.sh`; MiSTer stays
  capped at `GLIBC_2.31`.

Pre-squash verification head:

- Commit: `f98fe8ed1471d77b885097b6ecf9a80067e45551`
  (`Reject BuildKit cross image cache`).
- Verification run: `27492168313`, `workflow_dispatch`, success.
- Workflow wall: `2026-06-14T07:39:53Z` -> `2026-06-14T07:42:12Z`
  (`2m19s` by workflow timestamps; slowest job `2m14s`).
- The best observed accepted-state samples were around `1m56s`-`2m05s`; the
  final verification was slower but still much faster than the pre-branch `main`
  baseline (`3m48s` workflow wall, slowest job `3m43s`).
- The merge-ready branch was later squashed to a single commit,
  `Speed up Rust ARM CI with build caches`, with the same final file contents.

Rejected notable alternatives:

- Docker BuildKit `type=gha` image cache: valid but tied/slowed the accepted
  state and added Docker action overhead.
- Docker image tar cache: `docker load` cost erased the benefit.
- `Swatinem/rust-cache`: valid and incremental-disabled, but larger/slower than
  the manual broad target caches.
- Target-triple-only Rust cache: much smaller archives, but slower full CI.
- Removing `make`, native `gcc`, or linker/toolchain pieces: failed honest
  builds.

## Bun Rust build notes

User asked what Bun has done to make its Rust builds fast after its Rust port.
Checked public Bun sources on `2026-06-14`:

- `scripts/build/rust.ts`:
  <https://raw.githubusercontent.com/oven-sh/bun/main/scripts/build/rust.ts>
- `scripts/build/cargo-config.ts`:
  <https://raw.githubusercontent.com/oven-sh/bun/main/scripts/build/cargo-config.ts>
- `Cargo.toml`:
  <https://raw.githubusercontent.com/oven-sh/bun/main/Cargo.toml>

Relevant Bun techniques:

- **Cargo is a Ninja edge, not the whole build.** Bun emits a `cargo build -p
  bun_bin` rule into Ninja and declares the Rust staticlib as a build output.
  The rule uses `restat`, so when Cargo no-ops and the archive is unchanged,
  Ninja can prune downstream work. Applicability here: low for GitHub Actions
  because our CI is just Cargo/cross, but high if we ever build the Main fork,
  Slint UI, generated assets, and host tools through one orchestrator.
- **One Rust static library for the ported graph.** Bun builds `bun_bin` as a
  `staticlib` so the larger C++/JSC link sees a single Rust archive. This keeps
  the Rust graph cohesive and avoids post-merge object hacks. Applicability
  here: low; `mister-magik-fb` is already a normal Rust binary.
- **Explicit target dir beside the main build dir.** Bun writes Cargo outputs
  under a build-specific `rust-target/`, sibling to other object directories.
  Applicability here: medium. Our CI target cache already caches
  `magik-gui/target/...`, but a dedicated `CARGO_TARGET_DIR` per CI shape could
  make cache paths less broad and easier to prune.
- **Pinned/installed toolchain is repaired before builds.** Bun runs
  `rustup toolchain install <toolchain> --force --component rust-src ...` before
  cross builds because partial toolchains caused missing-std failures; they note
  it is a tiny no-op when complete. Applicability here: maybe. Our current
  `rustup show` / `rustup target add` steps take 2-10s, but they are not the
  long pole.
- **Use `lld` explicitly.** Bun writes/generated Cargo config and per-build env
  to use `clang++` driving `lld`, and also passes `-Clink-arg=-fuse-ld=lld`.
  Applicability here: high enough to test. Our ARM cross linker is currently
  `arm-linux-gnueabihf-gcc`, likely using GNU ld. A `rust-lld` or clang/lld ARM
  linker experiment may reduce the release-device LTO/link tail, but must keep
  MiSTer glibc compatibility and pass `check-arm-shared-libs.sh`.
- **Profile split: production vs fast iteration.** Bun's `release` uses fat LTO
  + `codegen-units = 1`; `release-dev` uses thin LTO + 16 CGUs. Applicability
  here: already partly done (`release` thin LTO, `release-device` fat LTO,
  `release-fast-dev` no LTO/incremental).
- **Local-only parallel rustc frontend.** Bun uses nightly `-Zthreads=8` only
  outside CI because CI/release builds want deterministic output/diagnostics.
  Applicability here: maybe for local device deploys, not this CI goal.
- **PGO and cross-language LTO are production-performance tools, not CI-time
  tools.** Useful context, but not a direct CI-speed experiment.

Concrete follow-up experiments for this repo:

1. Test ARM `lld` linking inside `Dockerfile.cross-armv7` or via `rust-lld`,
   scoped to the CI branch, and verify binary shared libraries.
2. Try narrower `CARGO_TARGET_DIR` values per matrix job to reduce target-cache
   restore/save size and avoid irrelevant profile artifacts.
3. Do not cargo-cult Bun's Buildx-style container work. Our first Docker-layer
   experiment below is already showing that Docker image caching can easily cost
   more than it saves on GitHub-hosted runners.

## Baseline

Workflow: `Rust ARM`

Latest successful `main` run before this branch:

- Run: `27479576473`
- Commit: `d77e4d2b1903e1a60efa4418523a89c72ff48357`
- Workflow wall time from `gh run list`: `3m48s`
- Date: `2026-06-13T21:26:10Z`

Per-job timings from `gh run view 27479576473 --json jobs`:

| Job | Job duration | Dominant step | Step duration |
|-----|--------------|---------------|---------------|
| host-dev | 1m17s | Test host logic | 23s |
| fast | 2m59s | Build ARM binary | 2m44s |
| fast-video | 3m13s | Build ARM binary | 2m55s |
| device | 3m31s | Build ARM binary | 3m15s |
| device-video-full | 3m43s | Build ARM binary | 3m28s |

Notes from logs:

- Cargo registry caching is present for host and Linux jobs.
- `cross` binary caching is present for Linux jobs.
- Minimal FFmpeg caching is present for video jobs.
- Compiled Cargo `target` outputs are not cached, so every CI run recompiles the
  Rust graph for every job.
- Post-job registry cache saves contend between Linux matrix jobs, but the
  failure is harmless because another matrix job wins the same primary key.

## Experiment 1: cache compiled Cargo outputs

Commit: `a6be6ce`

Change:

- Add `actions/cache@v4` for host debug outputs:
  `magik-gui/target/debug` and `tools/mister/target/debug`.
- Add per-matrix ARM output caches:
  `magik-gui/target/<profile>` and
  `magik-gui/target/armv7-unknown-linux-gnueabihf/<profile>`.
- Keep all existing build/test/check commands unchanged.

Hypothesis:

- First run may be neutral or slower because caches are populated.
- Subsequent PR runs should reduce repeated Rust compilation time, especially
  when workflow edits or small Rust changes do not invalidate most dependencies.

Measure:

- First PR run after cache introduction.
- Second PR run with a notes-only or workflow-only update to test warm-cache
  behavior.
- Compare total wall time and `Build ARM binary` step duration for all four ARM
  jobs.

Result:

- Cold cache-population run: `27479793933`
- Workflow wall time: `3m37s`, 11s faster than baseline `3m48s`.
- Conclusion: success.

| Job | Baseline job | Cold-cache job | Baseline dominant step | Cold-cache dominant step |
|-----|--------------|----------------|------------------------|--------------------------|
| host-dev | 1m17s | 1m09s | Test host logic 23s | Test host logic 16s |
| fast | 2m59s | 3m19s | Build ARM binary 2m44s | Build ARM binary 2m52s |
| fast-video | 3m13s | 3m10s | Build ARM binary 2m55s | Build ARM binary 2m37s |
| device | 3m31s | 3m31s | Build ARM binary 3m15s | Build ARM binary 3m09s |
| device-video-full | 3m43s | 3m22s | Build ARM binary 3m28s | Build ARM binary 2m53s |

Post-job target cache save costs on the cold run:

| Job | Post target-cache save |
|-----|------------------------|
| host-dev | 12s |
| fast | 14s |
| fast-video | 10s |
| device | 4s |
| device-video-full | 4s |

Interpretation:

- The first run already improved total wall time because the slowest job moved
  from `device-video-full` at 3m43s to `device` at 3m31s.
- The plain `fast` job regressed in total job time because it paid a 14s cache
  save and its compile step was 8s slower than baseline.
- The useful test is still the next run. If target caches restore correctly, the
  post-job save should mostly disappear and compile steps should drop.

## Experiment 1b: warm-cache measurement

Commit: `7a58ddf`

Change:

- Timing notebook update only. This should exercise the same CI commands with
  the target caches from run `27479793933` available for restore.

Result:

- Warm-cache run: `27479902828`
- Workflow wall time: `2m11s`, 1m37s faster than baseline `3m48s` and 1m26s
  faster than cold-cache run `27479793933`.
- Conclusion: success.

| Job | Baseline job | Cold-cache job | Warm-cache job | Baseline build/check step | Warm-cache build/check step |
|-----|--------------|----------------|----------------|---------------------------|-----------------------------|
| host-dev | 1m17s | 1m09s | 28s | Test host logic 23s | Test host logic 4s |
| fast | 2m59s | 3m19s | 1m35s | Build ARM binary 2m44s | Build ARM binary 1m18s |
| fast-video | 3m13s | 3m10s | 1m54s | Build ARM binary 2m55s | Build ARM binary 1m30s |
| device | 3m31s | 3m31s | 1m56s | Build ARM binary 3m15s | Build ARM binary 1m33s |
| device-video-full | 3m43s | 3m22s | 2m06s | Build ARM binary 3m28s | Build ARM binary 1m41s |

Post-job target cache behavior:

- Host and ARM target caches were primary-key hits, so post-job target cache
  save was 0-1s instead of the cold run's 4-14s.

Interpretation:

- Target caching is a keeper. The slowest job moved from baseline
  `device-video-full` at 3m43s to warm-cache `device-video-full` at 2m06s.
- The improvement is largest on repeated CI runs with small source/doc/workflow
  changes. That matches the intended development loop for this PR.
- The remaining slowest step is still `Build ARM binary`, now 1m18s-1m41s
  depending on profile/features. Further experiments should target cross-job or
  cross-profile compiler reuse, Docker setup, or artifact overhead without
  removing any build variants.

## Experiment 2: cache the cross Docker image layers

Commit: `a25a0ed`

Change:

- Add `docker/setup-buildx-action@v3`.
- Add `docker/build-push-action@v6` before target restore/build in each ARM
  matrix job.
- Build and load `magik-gui/Dockerfile.cross-armv7` as the same
  `MISTER_CROSS_IMAGE` tag that `cross` already uses.
- Use the GitHub Actions BuildKit cache backend with scope `cross-armv7`.

Hypothesis:

- Warm-cache run `27479902828` still spent about 35s in `device-video-full`
  before Cargo started:
  - `21:40:15`: `build-arm.sh` starts.
  - `21:40:17`: Docker build begins.
  - `21:40:18`: `apt-get update && apt-get install ...` begins.
  - `21:40:49`: Cargo starts compiling local crates.
- Prebuilding/loading the image with cached Docker layers should move or remove
  most of that apt/install cost from `Build ARM binary`.
- First run may be neutral or slower while the Docker cache is populated. The
  second run is the useful measurement.

Measure:

- Cross Docker image warm step duration.
- `Build ARM binary` step duration.
- Cargo-reported build duration inside the step.
- Total workflow wall time.

Result:

- Seed run: `27480024315`
- Workflow wall time: `3m54s`, 1m43s slower than target-cache-only warm run
  `27479902828` and 6s slower than the original baseline.
- Conclusion: success, but performance regression on the seed run.

| Job | Target-cache warm job | Docker seed job | Buildx setup | Warm cross Docker image | Build ARM binary |
|-----|-----------------------|-----------------|--------------|-------------------------|------------------|
| host-dev | 28s | 38s | n/a | n/a | n/a |
| fast | 1m35s | 3m10s | 6s | 1m24s | 1m16s |
| fast-video | 1m54s | 3m12s | 8s | 1m20s | 1m21s |
| device | 1m56s | 3m40s | 9s | 1m33s | 1m33s |
| device-video-full | 2m06s | 3m49s | 9s | 1m16s | 2m02s |

Interpretation:

- The seed run is not acceptable on its own. The Buildx setup plus image warm
  step adds 82-102s before the normal target restore/build path.
- `Build ARM binary` did not improve on the seed run; in the slowest job it
  regressed from 1m41s to 2m02s.
- Run one warm follow-up. If `Warm cross Docker image` does not drop to a small
  cached load and total wall time does not beat `2m11s`, revert experiment 2.

## Experiment 2b: warm Docker-layer measurement

Commit: `eabe7c7`

Change:

- Timing notebook update only. This should run with the BuildKit `gha` cache
  populated by `27480024315`.

Result:

- Warm Docker-cache run: `27480150005`
- Workflow wall time: `3m07s`, 56s slower than target-cache-only warm run
  `27479902828`.
- Conclusion: success, but rejected for performance.

| Job | Target-cache warm job | Docker warm job | Buildx setup | Warm cross Docker image | Build ARM binary |
|-----|-----------------------|-----------------|--------------|-------------------------|------------------|
| host-dev | 28s | 25s | n/a | n/a | n/a |
| fast | 1m35s | 1m56s | 8s | 22s | 1m09s |
| fast-video | 1m54s | 2m19s | 6s | 21s | 1m33s |
| device | 1m56s | 3m03s | 6s | 35s | 1m53s |
| device-video-full | 2m06s | 2m54s | 9s | 33s | 1m38s |

Decision:

- Revert experiment 2 from the workflow. Even warm, Docker image caching adds
  too much fixed cost on GitHub-hosted runners.
- Keep the target-output caches from experiment 1.
- Keep this result in the notebook because it explains why not to cargo-cult
  Docker layer caching here.

## Post-revert validation

Commit: `c346d25`

Change:

- Removed the Docker Buildx/image warm steps from the workflow.
- Kept target-output caching and this timing notebook.

Result:

- Run: `27480262551`
- Workflow wall time: `2m16s`
- Conclusion: success.

Interpretation:

- Reverting Docker layer caching restored the useful behavior from experiment 1.
- The remaining accepted workflow change is compiled Cargo target caching.

## Experiment 3: ARM lld linker

Commit: `ef0fb2f`

Change:

- Add `lld` to the ARM cross Docker image.
- Add opt-in `MISTER_ARM_USE_LLD=1` support in `magik-gui/build-arm.sh`.
- Enable `MISTER_ARM_USE_LLD=1` in CI.
- The script keeps the same `arm-linux-gnueabihf-gcc` linker driver, but passes
  `-C link-arg=-fuse-ld=lld`, matching Bun's general pattern of driving lld
  through the C/C++ linker driver.

Hypothesis:

- Bun explicitly routes Rust links through clang/lld. Our warm CI builds still
  spend 1m09s-1m53s in `Build ARM binary`, and release-device/fat-LTO jobs are
  likely link-heavy.
- `lld` may reduce the link/LTO tail, especially for `device` and
  `device-video-full`.

Risk:

- The Dockerfile change invalidates the target-cache key, so the first run will
  be a cold/seeding run and is not the final performance signal.
- `arm-linux-gnueabihf-gcc -fuse-ld=lld` may fail if the cross GCC cannot find
  the right `ld.lld`; if so, revert quickly.
- Must pass `check-arm-shared-libs.sh`; no dependency-shape cheating.

Measure:

- First run: correctness and cache-seeding cost.
- Second run: warm target-cache performance versus accepted post-revert run
  `27480262551` (`2m16s`) and target-cache warm run `27479902828` (`2m11s`).

Result:

- Run: `27480432476`
- Workflow wall time: `3m18s`
- Conclusion: failure.

Failure:

- All ARM matrix jobs failed in `Build ARM binary`.
- Representative `fast` log:
  - `==> using ARM linker: lld via arm-linux-gnueabihf-gcc -fuse-ld=lld`
  - `error: linking with arm-linux-gnueabihf-gcc failed`
  - `collect2: fatal error: cannot find 'ld'`

Decision:

- Revert experiment 3 from the workflow, Dockerfile, and build script.
- Bun's lld pattern is still interesting, but this simple
  `arm-linux-gnueabihf-gcc -fuse-ld=lld` transplant does not work in the
  current Ubuntu 20.04 cross image.
- A future lld attempt would need a proper linker-driver setup, probably either
  clang with `--target=arm-linux-gnueabihf --sysroot=/usr/arm-linux-gnueabihf`
  or explicit `ld.lld` path/search-dir wiring. That is larger than a quick CI
  timing experiment.

## Post-lld-revert validation

Commit: `52456a0`

Change:

- Removed the lld env, package, and build-script flag.
- Kept target-output caching and this timing notebook.

Result:

- Run: `27480557683`
- Workflow wall time: `2m18s`
- Conclusion: success.

Interpretation:

- PR checks are green again.
- Accepted state remains target-output caching only.

## Experiment 4: cache Docker image tar

Commit: `4e95b59`

Change:

- Add an `actions/cache@v4` entry for
  `/tmp/mister-cross-image/cross-armv7.tar`, keyed by
  `magik-gui/Dockerfile.cross-armv7`.
- On cache hit, run `docker load --input /tmp/mister-cross-image/cross-armv7.tar`.
- On cache miss, run the same Docker build that `cross` would perform and save
  the resulting `$MISTER_CROSS_IMAGE` with `docker save`.
- Keep all ARM matrix builds, host checks, shared-library checks, and artifact
  uploads unchanged.

Hypothesis:

- Buildx layer caching was rejected because the Buildx setup + cache/load path
  added too much fixed cost. A plain Docker image tar may be cheaper: one
  `actions/cache` restore plus `docker load`, no Buildx setup.
- If loaded layers are available, `cross` may still run its Docker build step
  but should hit local layer cache instead of doing `apt-get install`.

Risk:

- The image tar may be large enough that cache restore/load costs more than the
  repeated apt install.
- First run will be a seed run and likely slower while saving the tar.

Measure:

- `Cache cross Docker image`, `Load cached cross Docker image`, and
  `Build cross Docker image` step durations.
- Pre-Cargo time inside `Build ARM binary`.
- Total wall time versus accepted target-cache-only runs:
  `27479902828` (`2m11s`), `27480262551` (`2m16s`), `27480557683` (`2m18s`),
  and `27480407068` (`2m23s`).

Seed result:

- Run: `27480660488`
- Workflow wall time: `2m33s`
- Conclusion: success.
- `fast`: job `1m46s`; `Build cross Docker image` `37s`; `Build ARM binary`
  `45s`; post image cache `4s`.
- `fast-video`: job `2m13s`; `Build cross Docker image` `38s`; `Build ARM
  binary` `1m03s`; post image cache `2s`.
- `device`: job `2m20s`; `Build cross Docker image` `39s`; `Build ARM binary`
  `1m17s`; post image cache `2s`.
- `device-video-full`: job `2m22s`; `Build cross Docker image` `39s`; `Build
  ARM binary` `1m11s`; post image cache `3s`.

Seed interpretation:

- The seed run is slower than the accepted target-output-cache-only range
  (`2m11s` to `2m23s`), as expected for a cache-populating run.
- The per-job Docker image build costs about `37s` to `39s`; a warm run needs
  cache restore plus `docker load` to beat that by enough to offset any cache
  overhead.

Warm result:

- Run: `27480777191`
- Workflow wall time: `2m37s`
- Conclusion: success, but slower than accepted target-cache-only runs.
- `fast`: job `1m48s`; image cache restore `3s`; `docker load` `11s`;
  `Build ARM binary` `1m17s`.
- `fast-video`: job `2m14s`; image cache restore `5s`; `docker load` `10s`;
  `Build ARM binary` `1m27s`.
- `device`: job `2m00s`; image cache restore `8s`; `docker load` `20s`;
  `Build ARM binary` `1m13s`.
- `device-video-full`: job `2m32s`; image cache restore `5s`; `docker load`
  `21s`; `Build ARM binary` `1m35s`.

Decision:

- Reject experiment 4 and remove the Docker image tar cache steps.
- Warm restore plus `docker load` beat the explicit `docker build` step in
  isolation, but the full workflow still regressed to `2m37s`.
- The accepted PR state remains target-output caching only.

## Post-Docker-tar-revert validation

Commit: `7f660c1`

Change:

- Removed the Docker image tar cache, load, and prebuild steps.
- Kept target-output caching and the timing notebook.

Result:

- Run: `27480868880`
- Workflow wall time: `2m10s`
- Conclusion: success.
- `fast`: job `1m39s`; `Build ARM binary` `1m18s`.
- `fast-video`: job `1m42s`; `Build ARM binary` `1m22s`.
- `device`: job `1m57s`; `Build ARM binary` `1m39s`.
- `device-video-full`: job `2m06s`; `Build ARM binary` `1m31s`.

Interpretation:

- This is the best observed PR run so far, narrowly beating `27479902828`
  (`2m11s`).
- Accepted state remains target-output caching only.

## Experiment 5: artifact upload compression level 0

Commit: `06c1153`

Change:

- Set `compression-level: 0` on both `actions/upload-artifact@v4` uploads in
  each ARM matrix job:
  - `mister-magik-fb-${{ matrix.name }}`
  - `binary-size-${{ matrix.name }}`
- Keep all builds, host checks, shared-library checks, and artifact uploads.

Hypothesis:

- The ARM binaries and tiny TSVs are not worth spending CPU time compressing
  during CI.
- The uploads currently cost only about `1s` to `2s` each, so any win is likely
  small, but the change is low risk and preserves all outputs.

Measure:

- `Upload ARM binary`, `Upload size history`, and total workflow wall time
  versus the current accepted best `27480868880` (`2m10s`).

Result:

- Run: `27480943646`
- Workflow wall time: `2m22s`
- Conclusion: success, but slower than accepted best `27480868880` (`2m10s`).
- `fast`: job `1m30s`; `Upload ARM binary` `1s`; `Upload size history` `1s`;
  `Build ARM binary` `1m12s`.
- `fast-video`: job `1m45s`; `Upload ARM binary` `1s`; `Upload size history`
  `<1s`; `Build ARM binary` `1m20s`.
- `device`: job `2m04s`; `Upload ARM binary` `1s`; `Upload size history`
  `1s`; `Build ARM binary` `1m40s`.
- `device-video-full`: job `2m17s`; `Upload ARM binary` `1s`; `Upload size
  history` `1s`; `Build ARM binary` `1m54s`.

Decision:

- Reject experiment 5 and remove `compression-level: 0`.
- Upload steps were already tiny, and the end-to-end workflow regressed to
  `2m22s`.
- Accepted state remains target-output caching only.

## Post-artifact-compression-revert validation

Commit: `9ad4cc1`

Change:

- Removed `compression-level: 0` from artifact uploads.
- Kept target-output caching and the timing notebook.

Result:

- Run: `27481021209`
- Workflow wall time: `2m22s`
- Conclusion: success.
- `fast`: job `1m17s`; `Build ARM binary` `54s`.
- `fast-video`: job `1m44s`; `Build ARM binary` `1m18s`.
- `device`: job `2m12s`; `Build ARM binary` `1m52s`.
- `device-video-full`: job `2m11s`; `Build ARM binary` `1m48s`.

Interpretation:

- The accepted workflow is green again, but this run landed at `2m22s`.
- Run-to-run noise in Cargo/cross work is larger than the artifact upload
  compression tweak.

## Experiment 6: official prebuilt cross image

Commit: TBD

Change:

- Replace the custom `Dockerfile.cross-armv7` `cross` image config with the
  official pinned cross-rs image:
  `ghcr.io/cross-rs/armv7-unknown-linux-gnueabihf:0.2.5`.
- Set `MISTER_CROSS_IMAGE` to the same image so `build-minimal-ffmpeg.sh` and
  `check-arm-shared-libs.sh` use it too.
- Add an explicit `docker pull "$MISTER_CROSS_IMAGE"` step before the ARM build
  so helper scripts do not rebuild the custom Dockerfile.
- Pass `LIBCLANG_PATH=/usr/lib/llvm-3.8/lib` and the existing bindgen sysroot
  args into the `cross` container.

Hypothesis:

- Avoiding the custom Dockerfile build can remove the repeated `apt-get`
  image-build cost inside `Build ARM binary`.
- The official image already has the ARM gcc/sysroot, pkg-config, make, git,
  readelf/nm, and libclang, so it may be sufficient for Slint and FFmpeg.

Risk:

- The official cross image is older than the Ubuntu 20.04 custom image and only
  has libclang 3.8. Bindgen or FFmpeg may fail.
- Pulling the official image in every matrix job may cost enough to erase the
  Dockerfile-build win.

Measure:

- `Pull cross Docker image`, `Build ARM binary`, `Check ARM shared libraries`,
  and total workflow wall time versus accepted runs.

Result:

First attempt:

- Commit: `0fd7796`
- Run: `27481166124`
- Workflow wall time: failed in `56s`.
- `Pull cross Docker image` took `17s` to `18s` across ARM jobs.
- All ARM jobs failed in `Build ARM binary`.
- Representative `fast` failure:
  - `libsqlite3-sys` build script failed to execute.
  - `/lib/x86_64-linux-gnu/libc.so.6: version 'GLIBC_2.25' not found`
  - Similar missing symbols for `GLIBC_2.27`, `GLIBC_2.28`,
    `GLIBC_2.29`, and `GLIBC_2.30`.
  - A proc-macro `.so` then failed with missing `GLIBC_2.28`.

Interpretation:

- This is not yet a verdict on the official image itself.
- The target cache key still included `Dockerfile.cross-armv7` but not
  `Cross.toml` or the image choice, so CI restored build-script/proc-macro
  artifacts produced for the previous custom-image path.
- Those cached host artifacts are incompatible with the official cross image's
  older glibc.

Fix:

- Change the ARM target-output cache key to include `Cross.toml` and an explicit
  `cross-rs-0.2.5` image segment, forcing a cold target cache for this
  experiment.
- Tighten the restore key to the same `cross-rs-0.2.5` prefix so it cannot fall
  back to old custom-image artifacts.

Second attempt:

- Commit: `13422b8`
- Run: `27481213681`
- Workflow wall time: failed in `3m12s`.
- Cache fix worked: `fast` and `device` no longer failed on stale glibc-bound
  build-script artifacts.
- `fast`: success; `Pull cross Docker image` `15s`; `Build ARM binary`
  `2m13s`.
- `device`: success; `Pull cross Docker image` `16s`; `Build ARM binary`
  `2m36s`.
- `fast-video`: failed; `Pull cross Docker image` `17s`; `Build ARM binary`
  failed after `1m48s`.
- `device-video-full`: failed; `Pull cross Docker image` `17s`; `Build ARM
  binary` failed after `1m44s`.
- Representative video failure:
  - `/usr/arm-linux-gnueabihf/include/limits.h:123:16: fatal error:
    'limits.h' file not found`
  - `clang_getTranslationUnitTargetInfo` is not supported by loaded
    `libclang` `3.8.x`.

Decision:

- Reject experiment 6 and revert to the custom Ubuntu 20.04 cross image.
- The official cross-rs image can build non-video jobs after a clean target
  cache, but it does not satisfy the video/bindgen/libclang requirements.
- Even the successful non-video cold jobs were not promising enough to justify a
  more complex hybrid image path.
- Accepted state remains target-output caching only.

## Post-official-image-revert validation

Commit: `1d2ce22`

Change:

- Restored the custom Ubuntu 20.04 `Dockerfile.cross-armv7` path.
- Removed the explicit official-image pull step.
- Restored the accepted target-cache key shape.

Result:

- Run: `27481322254`
- Workflow wall time: `2m25s`
- Conclusion: success.
- `fast`: job `1m50s`; `Build ARM binary` `1m21s`.
- `fast-video`: job `2m16s`; `Build ARM binary` `1m47s`.
- `device`: job `2m00s`; `Build ARM binary` `1m41s`.
- `device-video-full`: job `2m20s`; `Build ARM binary` `2m02s`.

Interpretation:

- PR checks are green again.
- Accepted state remains target-output caching only.

## Experiment 7: trim cross Docker image packages

Commit: `1b0003a`

Change:

- Remove `clang`, `curl`, `file`, and `git` from
  `magik-gui/Dockerfile.cross-armv7`.
- Keep `libclang-dev`, host/ARM gcc/g++, `make`, `pkg-config`,
  `ca-certificates`, and the ARM sysroot packages.
- Keep all CI jobs, builds, checks, and artifact uploads unchanged.

Hypothesis:

- CI repeatedly builds the custom cross image inside `Build ARM binary`.
- Removing unused packages should reduce the `apt-get install` work without
  weakening any Rust, Slint, FFmpeg, or shared-library validation.

Risk:

- A transitive build script or FFmpeg configure path may rely on one of the
  removed tools.

Measure:

- `Build ARM binary` time and full workflow wall time versus the current
  accepted runs.

Result:

Seed run:

- Run: `27481453770`
- Workflow wall time: `2m47s`
- Conclusion: success, but expectedly slower than accepted warm target-cache
  runs because the Dockerfile hash changed and forced a new target-cache key.
- `fast`: job `2m16s`; `Build ARM binary` `1m46s`; Docker package
  install layer `21.9s`; Cargo build `1m14s`.
- `fast-video`: job `2m28s`; `Build ARM binary` `1m55s`.
- `device`: job `2m34s`; `Build ARM binary` `2m14s`; Docker package install
  layer `25.0s`; Cargo build `1m39s`.
- `device-video-full`: job `2m30s`; `Build ARM binary` `2m11s`; Cargo build
  `53.31s` after the helper image was cached by the earlier FFmpeg build path.

Interpretation:

- Removing the packages did not break the Rust, Slint, FFmpeg, video, or shared
  library checks.
- The seed run cannot prove a win because it paid the new target-cache-key tax.
- Need a warm follow-up with the same Dockerfile hash before deciding whether to
  keep or reject the trim.

Warm run:

- Commit: `fb3404e`
- Run: `27481544787`
- Workflow wall time: `2m14s`
- Conclusion: success.
- `fast`: job `1m39s`; `Build ARM binary` `1m14s`; Docker package install
  layer `22.9s`; Cargo build `38.83s`.
- `fast-video`: job `1m31s`; `Build ARM binary` `1m13s`.
- `device`: job `1m52s`; `Build ARM binary` `1m35s`.
- `device-video-full`: job `2m10s`; `Build ARM binary` `1m49s`.

Interpretation:

- Docker layers are not warm across GitHub-hosted jobs; every ARM job still
  rebuilds the custom image. The check step reuses the image inside the same
  job only.
- This run is better than the latest accepted validation (`27481322254`,
  `2m25s`) and close to the best accepted run (`27480868880`, `2m10s`), but the
  delta is small enough that it needs another warm validation before accepting.

Second warm validation:

- Commit: `b58b5c5`
- Run: `27481622766`
- Workflow wall time: `2m10s`
- Conclusion: success.
- `fast`: job `1m41s`; `Build ARM binary` `1m21s`.
- `fast-video`: job `1m31s`; `Build ARM binary` `1m12s`.
- `device`: job `2m00s`; `Build ARM binary` `1m39s`.
- `device-video-full`: job `2m06s`; `Build ARM binary` `1m43s`.

Decision:

- Accept the package trim as a small low-risk win.
- It does not solve the larger Docker image rebuild cost, but two warm runs
  (`2m14s`, `2m10s`) are at least as good as the accepted target-cache-only
  range and improve over the latest pre-trim validation (`2m25s`).
- Accepted state is now target-output caching plus the slimmer custom cross
  image package list.

Post-accept validation:

- Commit: `b474332`
- Run: `27481682104`
- Workflow wall time: `2m25s`
- Conclusion: success.
- Interpretation: green, but noisy; the trim is small enough that the accepted
  range should be treated as roughly `2m10s` to `2m25s`.

## Experiment 8: cheap build scripts and proc macros

Commit: `3ae5926`

Change:

- Add `build-override` sections for `release` and `release-device`:
  `opt-level = 0`, `codegen-units = 256`.
- Keep final ARM binary profiles, features, matrix jobs, artifact uploads, host
  checks, and shared-library checks unchanged.

Hypothesis:

- Warm CI runs still recompile local build-time units such as build scripts and
  proc macros after notes-only commits.
- Making those host-side units cheap to compile may reduce rebuild time without
  changing target binary optimization.

Risk:

- Some build scripts may run slightly slower when compiled with `opt-level = 0`.
- Cargo may already compile build-time units cheaply enough, making this neutral.

Measure:

- `Build ARM binary` times and total workflow wall time versus the accepted
  post-trim range: `2m10s`, `2m14s`, and noisy `2m25s`.

Result:

- Run: `27481763082`
- Workflow wall time: `3m54s`
- Conclusion: success.
- Job timings:
  - `fast`: `3m14s`, `Build ARM binary` `2m50s`
  - `fast-video`: `3m50s`, `Build ARM binary` `3m19s`
  - `device`: `3m43s`, `Build ARM binary` `3m15s`
  - `device-video-full`: `3m23s`, `Build ARM binary` `2m56s`
- Decision: reject. This is much slower than the accepted warm range
  (`2m10s` to `2m25s`) and inflates the ARM build step across every matrix leg.
  The likely explanation is that slower host build-script/proc-macro execution,
  combined with Cargo's rebuild graph after the profile change, costs more than
  any cheaper host codegen saves.

Revert:

- Commit: `c4e67de`
- Run: `27481895652`
- Workflow wall time: `2m03s`
- Conclusion: success.
- Job timings:
  - `fast`: `1m37s`, `Build ARM binary` `1m16s`
  - `fast-video`: `1m38s`, `Build ARM binary` `1m18s`
  - `device`: `1m51s`, `Build ARM binary` `1m35s`
  - `device-video-full`: `1m59s`, `Build ARM binary` `1m40s`
- Interpretation: accepted workflow recovered immediately after the revert.

## Experiment 9: skip host target install for cross builds

Commit: TBD

Change:

- In ARM matrix jobs, run only `rustup show` during the Rust toolchain step.
- Remove `rustup target add armv7-unknown-linux-gnueabihf` from CI setup.
- Keep `cross build`, all four ARM matrix entries, shared-library checks,
  artifact uploads, and host checks unchanged.

Hypothesis:

- `cross` performs the ARM build inside its container and should not require the
  host runner to install the ARM Rust std component.
- Avoiding the redundant host target install may shave a few seconds from every
  ARM job's setup step.

Risk:

- If `cross` expects the host target std component for this configuration, ARM
  builds will fail quickly.

Measure:

- `Install Rust toolchain` step time, total job time, and full workflow wall
  time versus the restored accepted validation run `27481895652` (`2m03s`).

Result:

- Commit: `c233282`
- Run: `27481956220`
- Workflow wall time: `2m15s`
- Conclusion: success.
- Job timings:
  - `fast`: `1m26s`, `Install Rust toolchain` `3s`, `Build ARM binary` `1m10s`
  - `fast-video`: `1m37s`, `Install Rust toolchain` `3s`, `Build ARM binary` `1m08s`
  - `device`: `1m59s`, `Install Rust toolchain` `2s`, `Build ARM binary` `1m33s`
  - `device-video-full`: `2m12s`, `Install Rust toolchain` `3s`,
    `Build ARM binary` `1m49s`
- Decision: reject. `cross` does not require the host target component, but the
  setup step remained about the same and the full workflow regressed versus the
  `2m03s` restored validation. The tiny fast-profile improvement was not enough
  to offset slower device/video timing.

Revert:

- Commit: `b9b61e6`
- Validation run: `27482041892`
- Workflow wall time: `2m11s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `38s`
  - `fast`: `1m41s`, `Build ARM binary` `1m21s`
  - `fast-video`: `1m43s`, `Build ARM binary` `1m18s`
  - `device`: `2m01s`, `Build ARM binary` `1m41s`
  - `device-video-full`: `2m08s`, `Build ARM binary` `1m45s`

## Experiment 10: Enable sccache for cross Rust builds

Bun research note:

- Bun's Rust build work is careful about avoiding repeated work: Cargo is wired
  into Ninja as a `restat` edge, has an explicit target directory, and release
  builds keep profile/link behavior deliberate. Their local-only
  `-Zthreads=8` idea is not suitable for this CI because it needs nightly and
  Bun keeps it out of CI/release for determinism.
- The closest safe analogue in this repo is a compiler-output cache. The repo
  already had dormant `Cross.sccache.toml` and a sccache Dockerfile, so this
  experiment revives that path instead of changing what gets built.

Change:

- Use `Dockerfile.cross-armv7-sccache` as the active cross image and set
  `rustc-wrapper = "sccache"` in `Cross.toml`.
- Slim the sccache image package list to match the accepted cross image, keeping
  only `curl`/`tar` extra for installing sccache.
- Cache `magik-gui/target/sccache` in Actions with matrix-specific keys.

Hypothesis:

- Cold run may regress because the image/cache key changes and sccache has to
  populate.
- Warm run might reduce `Build ARM binary` time after source/doc-only changes,
  especially for the slower device/video profiles.

Risk:

- The sccache wrapper may add overhead on top of already-restored Cargo target
  outputs, or the `/target/sccache` path may not persist correctly through
  `cross`.

Measure:

- Seed run and at least one warm run. Compare workflow wall time and per-job
  `Build ARM binary` time to restored accepted validation `27482041892` (`2m11s`).

Result:

- Seed commit: `004168a`
- Seed run: `27482125650`
- Workflow wall time: `2m36s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `38s`
  - `fast`: `2m20s`, `Build ARM binary` `1m21s`,
    `Check ARM shared libraries` `28s`
  - `fast-video`: `2m08s`, `Build ARM binary` `1m10s`,
    `Check ARM shared libraries` `29s`
  - `device`: `2m21s`, `Build ARM binary` `1m35s`,
    `Check ARM shared libraries` `26s`
  - `device-video-full`: `2m33s`, `Build ARM binary` `1m44s`,
    `Check ARM shared libraries` `27s`
- Early read: compile steps are not worse and sometimes slightly faster, but
  job wall time regressed because `Check ARM shared libraries` now spends
  ~26-29s in helper-image work. The likely cause is using a separate sccache
  Dockerfile for `cross` while the shared-library script still builds/uses the
  non-sccache helper image.

Warm result:

- Warm commit: `c782dce`
- Warm run: `27482192439`
- Workflow wall time: `2m45s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `46s`
  - `fast`: `2m10s`, `Build ARM binary` `1m15s`,
    `Check ARM shared libraries` `28s`
  - `fast-video`: `2m08s`, `Build ARM binary` `1m16s`,
    `Check ARM shared libraries` `28s`
  - `device`: `2m37s`, `Build ARM binary` `1m42s`,
    `Check ARM shared libraries` `27s`
  - `device-video-full`: `2m41s`, `Build ARM binary` `1m46s`,
    `Check ARM shared libraries` `26s`
- Decision: reject this shape. The warm run is slower than the restored
  accepted validation `27482041892` (`2m11s`) and slower than the accepted
  target-cache-only range. sccache did not overcome the extra image/cache path;
  it also made the library check consistently expensive by splitting the active
  cross image from the helper-image path.
- Follow-up idea: if sccache is revisited, test a single-image variant that
  installs sccache into the main `Dockerfile.cross-armv7` so `cross`, minimal
  FFmpeg, and `check-arm-shared-libs.sh` share the same helper image.

Revert:

- Restore `Cross.toml` to `Dockerfile.cross-armv7`.
- Remove the Actions sccache cache and sccache env.
- Restore the target-cache key to the accepted non-sccache Dockerfile input.
- Commit: `521f77d`
- Validation run: `27482296535`
- Workflow wall time: `2m23s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `35s`
  - `fast`: `1m32s`, `Build ARM binary` `1m11s`,
    `Check ARM shared libraries` `0s`
  - `fast-video`: `1m42s`, `Build ARM binary` `1m22s`,
    `Check ARM shared libraries` `0s`
  - `device`: `2m00s`, `Build ARM binary` `1m32s`,
    `Check ARM shared libraries` `1s`
  - `device-video-full`: `2m19s`, `Build ARM binary` `1m54s`,
    `Check ARM shared libraries` `0s`
- Interpretation: accepted workflow recovered to the established warm-cache
  band. The shared-library check returned to ~0-1s because `cross` and the check
  script share the same active helper image again.

Second restored-baseline confirmation:

- Note-only commit after the sccache revert: `2bfac40`
- Run: `27482363079`
- Workflow wall time: `2m38s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `25s`
  - `fast`: `1m33s`, `Build ARM binary` `1m10s`,
    `Check ARM shared libraries` `1s`
  - `fast-video`: `1m39s`, `Build ARM binary` `1m13s`,
    `Check ARM shared libraries` `1s`
  - `device`: `1m57s`, `Build ARM binary` `1m34s`,
    `Check ARM shared libraries` `1s`
  - `device-video-full`: `2m32s`, `Build ARM binary` `1m58s`,
    `Check ARM shared libraries` `1s`
- Interpretation: still green, but at the slower end of the accepted warm-cache
  band. Use the accepted range, not only the best run, when judging a marginal
  experiment.

## Experiment 11: host Cargo with Ubuntu ARM cross toolchain

Change:

- Add a CI-only `MISTER_CI_HOST_CARGO=1` path in `build-arm.sh` that uses host
  `cargo build --target armv7-unknown-linux-gnueabihf` with the same profile,
  features, Rust flags, binary size recording, and output paths.
- Install the ARM linker/binutils/libclang packages directly on the Ubuntu
  runner instead of installing/caching the `cross` binary.
- Let `check-arm-shared-libs.sh` use host `arm-linux-gnueabihf-readelf` when
  available, falling back to the Docker helper only when needed.
- Keep all four ARM matrix builds, host checks, shared-library checks, and
  artifact uploads.

Hypothesis:

- Warm-cache runs still spend about 25-28s per ARM job building/exporting the
  custom `cross` Docker image before Cargo starts.
- A host toolchain apt step may cost a similar amount up front, but it removes
  Docker build/export and `cross` wrapper overhead from the hot `Build ARM
  binary` step. If apt is not worse than Docker image construction, total wall
  time may improve or at least clarify whether Docker is worth keeping.

Risk:

- Host Ubuntu's `libclang` version/path differs from the Ubuntu 20.04 cross
  image; bindgen or ffmpeg-sys may fail if the environment is incomplete.
- Video jobs can still need the Docker helper on a cold FFmpeg cache rebuild,
  so this mainly measures the normal warm-cache path.

Measure:

- Compare full workflow wall time and `Build ARM binary` durations to the
  restored accepted run `27482296535` (`2m23s`).

Seed result:

- Commit: `af18cd0`
- Run: `27482420335`
- Workflow conclusion: failure.
- Workflow wall time until failure/success completion: about `3m40s`
  (`23:35:26` to `23:39:06` on the longest successful job).
- Job timings:
  - `host-dev`: `19s`
  - `fast`: `3m03s`, `Build ARM binary` `2m26s`,
    `Check ARM shared libraries` `0s`
  - `fast-video`: failed after `2m39s`, `Build ARM binary` failed after `1m56s`
  - `device`: `3m40s`, `Build ARM binary` `2m50s`,
    `Check ARM shared libraries` `0s`
  - `device-video-full`: failed after `2m31s`, `Build ARM binary` failed after
    `1m46s`
- Failure cause: video jobs restored the cached minimal FFmpeg tree under
  `magik-gui/target/ffmpeg-minimal/armv7`, but `build-arm.sh` still exported
  Docker-internal paths (`/target/ffmpeg-minimal/armv7/dist`) for `FFMPEG_DIR`,
  `PKG_CONFIG_PATH`, and C include flags. Host Cargo therefore could not see
  `libavutil/avutil.h`.
- Early read: the non-video jobs succeeded but were slow because the target
  cache key changed with this experiment and produced a seed/miss run. Fix the
  host FFmpeg path and run again before deciding; if a warm fixed run does not
  beat the accepted `cross` band, reject.

Fix:

- Commit: `fea4af8`
- Change host-Cargo video builds to use the local
  `magik-gui/target/ffmpeg-minimal/armv7/dist` path for `FFMPEG_DIR`,
  `PKG_CONFIG_PATH`, and C include flags while keeping the Docker-internal
  `/target/...` path for normal `cross` builds.

Fixed seed result:

- Run: `27482531604`
- Workflow wall time: `3m43s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `24s`
  - `fast`: `1m28s`, `Build ARM binary` `42s`,
    `Check ARM shared libraries` `0s`
  - `fast-video`: `3m16s`, `Build ARM binary` `2m27s`,
    `Check ARM shared libraries` `1s`
  - `device`: `1m57s`, `Build ARM binary` `1m11s`,
    `Check ARM shared libraries` `0s`
  - `device-video-full`: `3m43s`, `Build ARM binary` `2m57s`,
    `Check ARM shared libraries` `0s`
- Interpretation: the host toolchain path can be much faster for non-video ARM
  jobs, and host `readelf` removes the shared-library Docker helper cost.
  However, video builds dominate wall time and are slower than accepted `cross`
  runs in this seed. Run one note-only warm validation with the same
  `build-arm.sh` hash before deciding whether video was cold-cache fallout.

Warm result:

- Note-only commit: `39736a3`
- Run: `27482638172`
- Workflow wall time: `2m07s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `23s`
  - `fast`: `1m13s`, `Build ARM binary` `41s`,
    `Check ARM shared libraries` `0s`
  - `fast-video`: `1m42s`, `Build ARM binary` `49s`,
    `Check ARM shared libraries` `0s`
  - `device`: `1m45s`, `Build ARM binary` `1m02s`,
    `Check ARM shared libraries` `0s`
  - `device-video-full`: `2m02s`, `Build ARM binary` `1m19s`,
    `Check ARM shared libraries` `0s`
- Decision: accept pending at least one follow-up validation. This is the first
  experiment to beat the accepted target-cache-only warm range on full workflow
  wall time while keeping every host check, ARM profile, shared-library check,
  and artifact upload. The win comes from avoiding per-job `cross` image
  build/export work and replacing Docker-based shared-library inspection with
  host `arm-linux-gnueabihf-readelf`; the cost is a 16-20s apt install step per
  ARM job, but the warm build steps are enough faster to compensate.

Validation:

- Note-only commit: `fbb974b`
- Run: `27482720899`
- Workflow wall time: `2m05s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `33s`
  - `fast`: `1m26s`, `Install ARM cross toolchain` `25s`,
    `Build ARM binary` `47s`, `Check ARM shared libraries` `0s`
  - `fast-video`: `1m22s`, `Install ARM cross toolchain` `17s`,
    `Build ARM binary` `47s`, `Check ARM shared libraries` `0s`
  - `device`: `1m32s`, `Install ARM cross toolchain` `17s`,
    `Build ARM binary` `56s`, `Check ARM shared libraries` `0s`
  - `device-video-full`: `2m02s`, `Install ARM cross toolchain` `40s`,
    `Build ARM binary` `59s`, `Check ARM shared libraries` `0s`
- Final decision: accept. The validation backs up the warm result and establishes
  a new best full-workflow band (`2m05s`-`2m07s`) without dropping any builds,
  tests, shared-library checks, or artifact uploads.

Passive confirmation after recording validation:

- Note-only commit: `ffced75`
- Run: `27482819350`
- Workflow wall time: `1m56s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `21s`
  - `fast`: `1m23s`, `Install ARM cross toolchain` `20s`,
    `Build ARM binary` `35s`, `Check ARM shared libraries` `0s`
  - `fast-video`: `1m41s`, `Install ARM cross toolchain` `18s`,
    `Build ARM binary` `49s`, `Check ARM shared libraries` `0s`
  - `device`: `1m51s`, `Install ARM cross toolchain` `36s`,
    `Build ARM binary` `48s`, `Check ARM shared libraries` `0s`
  - `device-video-full`: `1m52s`, `Install ARM cross toolchain` `16s`,
    `Build ARM binary` `1m16s`, `Check ARM shared libraries` `0s`
- Interpretation: host-Cargo accepted state can land below two minutes when the
  apt step avoids the earlier outlier. This is the new best confirmed run.

## Experiment 12: trim host ARM apt packages

Change:

- Remove `g++-arm-linux-gnueabihf` from the Ubuntu ARM toolchain install step.
- Stop exporting `CXX_armv7_unknown_linux_gnueabihf` in the CI host-Cargo path.

Hypothesis:

- The accepted host-Cargo workflow now spends 17-40s per ARM job in `apt-get`
  toolchain setup. The project’s CI ARM path builds Rust plus C libraries
  (`libsqlite3-sys`, FFmpeg C code, bindgen checks) and links with GCC; it should
  not need an ARM C++ compiler.
- Removing the C++ cross compiler may reduce apt install time without changing
  the build/test/artifact surface.

Risk:

- Some transitive native dependency may invoke `CXX` only in video or full-scene
  profiles. If so, this should fail visibly in the normal ARM matrix and be
  reverted.

Measure:

- Compare the full workflow wall time and `Install ARM cross toolchain` step
  durations to accepted host-Cargo validation `27482720899` (`2m05s`).

Result:

- Commit: `456f8db`
- Run: `27482837107`
- Workflow wall time: `2m06s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `30s`
  - `fast`: `1m42s`, `Install ARM cross toolchain` `26s`,
    `Build ARM binary` `45s`, `Check ARM shared libraries` `0s`
  - `fast-video`: `1m34s`, `Install ARM cross toolchain` `21s`,
    `Build ARM binary` `50s`, `Check ARM shared libraries` `0s`
  - `device`: `1m44s`, `Install ARM cross toolchain` `17s`,
    `Build ARM binary` `1m07s`, `Check ARM shared libraries` `0s`
  - `device-video-full`: `2m02s`, `Install ARM cross toolchain` `24s`,
    `Build ARM binary` `1m16s`, `Check ARM shared libraries` `0s`
- Decision: reject. The matrix stays green, proving the C++ package is not
  required for this warm-cache path, but the workflow did not improve versus the
  accepted host-Cargo range and was slower than the immediately preceding
  accepted-state confirmation (`1m56s`). Restore the C++ package/env export so
  cold-cache/native-dependency edge cases keep the fuller toolchain.

Revert:

- Restore `g++-arm-linux-gnueabihf` and the host-Cargo
  `CXX_armv7_unknown_linux_gnueabihf` export.
- Commit: `9e85f32`
- Validation run: `27482925884`
- Workflow wall time: `1m58s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `39s`
  - `fast`: `1m13s`, `Install ARM cross toolchain` `18s`,
    `Build ARM binary` `40s`, `Check ARM shared libraries` `0s`
  - `fast-video`: `1m32s`, `Install ARM cross toolchain` `23s`,
    `Build ARM binary` `46s`, `Check ARM shared libraries` `0s`
  - `device`: `1m39s`, `Install ARM cross toolchain` `18s`,
    `Build ARM binary` `1m06s`, `Check ARM shared libraries` `0s`
  - `device-video-full`: `1m54s`, `Install ARM cross toolchain` `23s`,
    `Build ARM binary` `1m13s`, `Check ARM shared libraries` `0s`
- Interpretation: accepted host-Cargo state remains in the sub-two-minute band.

## Experiment 13: skip apt-get update for host ARM packages

Change:

- Keep the accepted host-Cargo package list, but remove `sudo apt-get update`
  from the CI toolchain install step.

Hypothesis:

- GitHub-hosted Ubuntu runners often have usable package indexes already. If so,
  this may cut the 16-40s `Install ARM cross toolchain` step without changing
  the toolchain, build matrix, tests, shared-library checks, or artifacts.

Risk:

- This may be flaky if hosted runner package indexes are absent or stale. Any
  package resolution failure should reject the experiment even if it would work
  after an update.

Measure:

- Compare full workflow wall time and toolchain install durations to accepted
  host-Cargo runs, especially `27482819350` (`1m56s`) and the revert validation
  run for `9e85f32` once it completes.

Result:

- Commit: `461681f`
- Run: `27482954910`
- Workflow wall time: `1m52s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `43s`
  - `fast`: `1m22s`, `Install ARM cross toolchain` `19s`,
    `Build ARM binary` `42s`, `Check ARM shared libraries` `0s`
  - `fast-video`: `1m22s`, `Install ARM cross toolchain` `11s`,
    `Build ARM binary` `46s`, `Check ARM shared libraries` `0s`
  - `device`: `1m31s`, `Install ARM cross toolchain` `15s`,
    `Build ARM binary` `1m01s`, `Check ARM shared libraries` `0s`
  - `device-video-full`: `1m48s`, `Install ARM cross toolchain` `10s`,
    `Build ARM binary` `1m16s`, `Check ARM shared libraries` `0s`
- Decision: accept pending repeat validation. This is the fastest full workflow
  so far, and it keeps the same package list and job surface. Because skipping
  `apt-get update` can be runner-image dependent, require a second green run
  before locking it in.

Validation:

- Note-only commit: `0c66f25`
- Run: `27483051391`
- Workflow wall time: `2m02s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `20s`
  - `fast`: `1m11s`, `Install ARM cross toolchain` `18s`,
    `Build ARM binary` `32s`, `Check ARM shared libraries` `0s`
  - `fast-video`: `1m32s`, `Install ARM cross toolchain` `11s`,
    `Build ARM binary` `1m03s`, `Check ARM shared libraries` `0s`
  - `device`: `1m40s`, `Install ARM cross toolchain` `11s`,
    `Build ARM binary` `1m08s`, `Check ARM shared libraries` `0s`
  - `device-video-full`: `1m57s`, `Install ARM cross toolchain` `19s`,
    `Build ARM binary` `1m14s`, `Check ARM shared libraries` `0s`
- Final decision: accept. The repeat is slower than the first no-update sample
  but still green, and the toolchain install step stayed shorter than the
  accepted host-Cargo runs that performed `apt-get update`. The new observed
  no-update band is `1m52s`-`2m02s`, compared with the prior host-Cargo accepted
  band of roughly `1m56s`-`2m07s`.

Passive confirmation after recording acceptance:

- Note-only commit: `c6f8832`
- Run: `27483121945`
- Workflow wall time: `1m58s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `31s`
  - `fast`: `1m23s`, `Install ARM cross toolchain` `22s`,
    `Build ARM binary` `42s`, `Check ARM shared libraries` `0s`
  - `fast-video`: `1m31s`, `Install ARM cross toolchain` `13s`,
    `Build ARM binary` `51s`, `Check ARM shared libraries` `0s`
  - `device`: `1m50s`, `Install ARM cross toolchain` `20s`,
    `Build ARM binary` `1m02s`, `Check ARM shared libraries` `0s`
  - `device-video-full`: `1m53s`, `Install ARM cross toolchain` `14s`,
    `Build ARM binary` `1m18s`, `Check ARM shared libraries` `0s`
- Interpretation: third no-update sample remains green and under two minutes.

## Experiment 14: artifact upload compression-level 0, retest

Change:

- Add `compression-level: 0` to both `actions/upload-artifact@v4` calls for the
  ARM binary and `build/binary-size.tsv`.

Hypothesis:

- A previous compression-level experiment was neutral under the older
  Docker/cross workflow. With the current best workflow near two minutes, upload
  compression may be visible enough to trim a few seconds without changing
  artifacts or job coverage.

Risk:

- The binaries are small enough that upload compression may not matter, and
  artifact transfer time may dominate. If wall time is not improved, reject.

Measure:

- Compare full workflow wall time and upload step durations to accepted
  no-update runs `27482954910` (`1m52s`), `27483051391` (`2m02s`), and
  `27483121945` (`1m58s`).

Result:

- Commit: `f86ae99`
- Run: `27483205222`
- Workflow wall time: `1m54s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `42s`
  - `fast`: `1m09s`, `Build ARM binary` `36s`, `Upload ARM binary` `2s`,
    `Upload size history` `1s`
  - `fast-video`: `1m25s`, `Build ARM binary` `47s`,
    `Upload ARM binary` `6s`, `Upload size history` `1s`
  - `device`: `1m36s`, `Build ARM binary` `1m03s`, `Upload ARM binary` `2s`,
    `Upload size history` `1s`
  - `device-video-full`: `1m50s`, `Build ARM binary` `1m16s`,
    `Upload ARM binary` `1s`, `Upload size history` `1s`
- Decision: accept pending repeat validation. Full workflow wall time is in the
  best no-update band, but upload durations are still noisy and not uniformly
  better; repeat before deciding.

Validation:

- Note-only commit: `a5a224d`
- Run: `27483279207`
- Workflow wall time: `2m11s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `19s`
  - `fast`: `1m10s`, `Build ARM binary` `44s`, `Upload ARM binary` `1s`,
    `Upload size history` `1s`
  - `fast-video`: `2m07s`, `Build ARM binary` `57s`,
    `Upload ARM binary` `1s`, `Upload size history` `1s`
  - `device`: `1m40s`, `Build ARM binary` `57s`, `Upload ARM binary` `7s`,
    `Upload size history` `1s`
  - `device-video-full`: `2m03s`, `Build ARM binary` `1m15s`,
    `Upload ARM binary` `2s`, `Upload size history` `1s`
- Decision: reject. The repeat is slower than the no-update accepted band and
  upload duration remains noisy. Restore default artifact compression settings.

Revert:

- Remove `compression-level: 0` from both artifact upload steps.
- Commit: `d216057`
- Validation run: `27483353454`
- Workflow wall time: `2m10s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `38s`
  - `fast`: `1m30s`, `Install Rust toolchain` `3s`
  - `fast-video`: `1m20s`, `Install Rust toolchain` `3s`
  - `device`: `1m35s`, `Install Rust toolchain` `3s`
  - `device-video-full`: `2m05s`, `Install Rust toolchain` `4s`
- Interpretation: artifact-compression revert restored the accepted workflow
  shape, but this sample is at the slower end of the no-update band.

## Experiment 15: remove rustup show from ARM jobs

Change:

- In the ARM matrix only, replace the two-line Rust setup step
  (`rustup show` plus `rustup target add armv7-unknown-linux-gnueabihf`) with
  just `rustup target add armv7-unknown-linux-gnueabihf`.

Hypothesis:

- `rustup target add` should already honor `magik-gui/rust-toolchain.toml` in
  that working directory and install/sync the selected stable toolchain as
  needed. Dropping `rustup show` may save a couple of seconds in every ARM job
  without changing the compiler, target, build profiles, checks, or artifacts.

Risk:

- If `rustup show` was doing necessary toolchain setup on the hosted runner,
  this will fail in the ARM matrix and should be reverted.

Measure:

- Compare full workflow wall time and ARM `Install Rust toolchain` step
  durations to the accepted no-update band, plus the artifact-revert validation
  run for `d216057` once it completes.

Result:

- Commit: `76c9171`
- Run: `27483378746`
- Workflow wall time: `1m55s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `47s`
  - `fast`: `1m15s`, `Install Rust toolchain` `2s`,
    `Build ARM binary` `45s`
  - `fast-video`: `1m23s`, `Install Rust toolchain` `2s`,
    `Build ARM binary` `35s`
  - `device`: `1m25s`, `Install Rust toolchain` `3s`,
    `Build ARM binary` `56s`
  - `device-video-full`: `1m50s`, `Install Rust toolchain` `3s`,
    `Build ARM binary` `1m14s`
- Decision: accept pending repeat validation. The ARM Rust setup step shrank
  from roughly 3-4s to 2-3s and the full workflow landed in the best accepted
  band. Repeat once because most of the wall-time win still comes from normal
  build/cache variance rather than the small setup-step change alone.

Validation:

- Note-only commit: `a4d30d9`
- Run: `27483462896`
- Workflow wall time: `2m19s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `30s`
  - `fast`: `1m14s`, `Install Rust toolchain` `2s`,
    `Build ARM binary` `41s`
  - `fast-video`: `1m30s`, `Install Rust toolchain` `3s`,
    `Build ARM binary` `46s`
  - `device`: `1m57s`, `Install Rust toolchain` `4s`,
    `Build ARM binary` `1m09s`
  - `device-video-full`: `2m15s`, `Install Rust toolchain` `5s`,
    `Build ARM binary` `1m28s`
- Decision: reject. The repeat stayed green but did not improve wall time, and
  the Rust setup step savings were too small/noisy to justify losing the useful
  `rustup show` diagnostic in CI logs. Restore the original two-line setup.

Revert:

- Restore `rustup show` before `rustup target add` in the ARM matrix.
- Commit: `58abd93`

Correctness audit:

- Downloaded `mister-magik-fb-device-video-full` from accepted host-Cargo run
  `27483121945`.
- Inspected dynamic symbol versions with the project ARM helper image:
  `arm-linux-gnueabihf-readelf --version-info`.
- Result: the host-Cargo artifact from Ubuntu 22.04 requires up to
  `GLIBC_2.35`.
- Device fact from `AGENTS.md`: MiSTer has glibc `2.31`.
- Conclusion: the Ubuntu 22.04 host-Cargo speed win is not acceptable by itself;
  the CI checks missed a runtime compatibility regression. Add a GLIBC symbol
  version guard to `check-arm-shared-libs.sh` so future timing experiments cannot
  pass with binaries newer than the MiSTer runtime.

## Experiment 16: host Cargo on Ubuntu 20.04 ARM runner

Change:

- Keep the host-Cargo workflow shape, but run the ARM matrix on `ubuntu-20.04`
  instead of `ubuntu-22.04`.
- Extend `check-arm-shared-libs.sh` to fail if the binary requires any GLIBC
  symbol newer than `GLIBC_2.31`.

Hypothesis:

- Ubuntu 20.04's ARM cross sysroot should match MiSTer's glibc 2.31 while
  preserving most of the host-Cargo speed win.
- If GitHub no longer supports `ubuntu-20.04`, or if the no-update apt path is
  not reliable there, the run will fail clearly.

Risk:

- `ubuntu-20.04` may be deprecated/unavailable on GitHub-hosted runners in 2026.
- The package-index shortcut from Experiment 13 may be less reliable on 20.04.
- The GLIBC guard adds another readelf pass, but that is honest and small.

Measure:

- Must pass all existing jobs plus the new GLIBC guard.
- Compare wall time to the original safe Docker/cross accepted band (`~2m10s` to
  `2m38s`) and to the fast but incompatible Ubuntu 22.04 host-Cargo band.

Interim result:

- Commit: `4925e7d`
- Run: `27483616717`
- `host-dev` completed successfully in `30s`.
- All four ARM matrix jobs remained queued with no steps at least through
  `2026-06-14T00:39:23Z`, while prior `ubuntu-22.04` ARM jobs normally started
  immediately.
- The run was cancelled at `2026-06-14T00:41:51Z` after the restored
  `ubuntu-22.04` cross run had already started ARM jobs promptly.
- Decision: reject. A glibc-compatible runner that cannot promptly start the ARM
  matrix is not a CI-time improvement. If GitHub-hosted `ubuntu-20.04`
  availability changes, this can be revisited, but it cannot be the accepted
  path tonight.

## Experiment 17: restore Docker/cross path with GLIBC guard

Change:

- Restore the ARM matrix to `ubuntu-22.04` plus the existing `cross` binary cache
  and Docker-based armv7 sysroot.
- Remove the active host-Cargo workflow path and its dormant build-script branch.
- Keep the new `check-arm-shared-libs.sh` GLIBC symbol-version guard.

Hypothesis:

- This should return to the last known safe Docker/cross timing band while
  closing the correctness hole that allowed Ubuntu 22.04 host-Cargo artifacts to
  require `GLIBC_2.35`.

Measure:

- Must pass all ARM profiles, artifact uploads, and the new GLIBC guard.
- Compare wall time to the pre-host-Cargo safe band (`2m10s`-`2m38s` after the
  slim Docker image and target-cache changes).

Result:

- Commit: `03d4175`
- Run: `27483760539`
- Workflow wall time: `2m11s`
- Conclusion: success.
- `device-video-full` shared-library log reported max GLIBC symbol version
  `GLIBC_2.31`, matching the MiSTer runtime ceiling.

Job timings:

| Job | Duration | Build ARM binary | Check ARM shared libraries | Notes |
|-----|----------|------------------|----------------------------|-------|
| host-dev | 34s | n/a | n/a | all host checks green |
| fast | 1m32s | 1m08s | 1s | GLIBC guard passed |
| fast-video | 1m41s | 1m21s | 0s | GLIBC guard passed |
| device | 1m48s | 1m33s | 1s | GLIBC guard passed |
| device-video-full | 2m07s | 1m45s | 1s | GLIBC guard passed; max `GLIBC_2.31` |

Interpretation:

- Restoring Docker/cross gives back the invalid Ubuntu 22.04 host-Cargo speed
  win, but it restores target-runtime correctness while staying near the best
  safe warm-cache band.
- The new GLIBC guard is cheap in CI (`0`-`1s`) and should stay. It caught the
  exact class of runtime regression that the previous shared-library check
  missed.
- The check step rebuilds/tags the same helper image name, but Docker layer cache
  makes that effectively free on this run. Further work should target build-step
  duration or cache restore size, not the guard.

Passive repeat:

- Note-only commit: `df4a6c7`
- Run: `27483826645`
- Workflow wall time: `2m28s`
- Conclusion: success.
- Job timings:
  - `host-dev`: `30s`
  - `fast`: `1m39s`, `Cache ARM build outputs` `4s`, `Build ARM binary` `1m16s`
  - `fast-video`: `1m55s`, `Cache ARM build outputs` `10s`,
    `Build ARM binary` `1m22s`
  - `device`: `2m18s`, `Cache ARM build outputs` `5s`,
    `Build ARM binary` `1m47s`
  - `device-video-full`: `2m17s`, `Cache ARM build outputs` `10s`,
    `Build ARM binary` `1m44s`
- Interpretation: still green and compatible, but this repeat is slower than
  `27483760539`. The ARM target-cache restore step is now visible enough
  (`4s`-`10s`) to justify a narrower cache-layout experiment.

## Experiment 18: per-matrix Cargo target dirs

Change:

- Set `CARGO_TARGET_DIR=target/ci/<matrix.name>` for each ARM matrix build.
- Cache only `magik-gui/target/ci/<matrix.name>` instead of broad profile
  directories under `magik-gui/target`.
- Teach `build-arm.sh` to find timing reports and the output binary through
  `CARGO_TARGET_DIR` when it is set.

Hypothesis:

- Narrower per-matrix target directories should reduce cache restore/save size
  and avoid unrelated profile artifacts, similar to Bun's explicit per-build
  Rust target directory layout.

Risk:

- The first run will be a seed run because the cache key/path changes.
- `cross` may not honor the relative `CARGO_TARGET_DIR` the same way plain Cargo
  does. If the binary path check fails, reject quickly.

Measure:

- Seed run should pass all jobs and upload artifacts from the new paths.
- A follow-up warm run is required before judging speed, because the cache path
  changes invalidate the accepted target caches.

Seed result:

- Commit: `633b6b9`
- Run: `27483911735`
- Conclusion: failure.
- Non-video jobs proved the new target dir path works:
  - `fast`: success in `3m14s`, `Build ARM binary` `2m47s`
  - `device`: success in `3m22s`, `Build ARM binary` `3m06s`
- Video jobs failed inside `ffmpeg-sys-next` build script:
  `fatal error: libavutil/avutil.h: No such file or directory`.
- Cause: with `CARGO_TARGET_DIR=target/ci/<job>`, `cross` maps that per-job
  directory to `/target`. The cached minimal FFmpeg build still lives under the
  project `target/ffmpeg-minimal`, so the old `/target/ffmpeg-minimal/...`
  include path no longer exists inside the container.

Follow-up fix:

- When `CARGO_TARGET_DIR` is set, point `FFMPEG_DIR` and C include flags at the
  project mount path `/project/target/ffmpeg-minimal/armv7/dist` instead of the
  container target path `/target/ffmpeg-minimal/armv7/dist`.

Follow-up result:

- Commit: `6026f78`
- Run: `27484015879`
- Conclusion: failure.
- Non-video jobs were green on the warmed per-matrix caches:
  - `host-dev`: success in `29s`
  - `fast`: success in `1m43s`, `Build ARM binary` `1m15s`
  - `device`: success in `2m02s`, `Build ARM binary` `1m16s`
- Video jobs still failed in the `ffmpeg-sys-next` C probe:
  - `fast-video`: failed in `2m28s`
  - `device-video-full`: failed in `2m23s`
- The failing logs showed `CFLAGS = ... -I/project/target/ffmpeg-minimal/armv7/dist/include`,
  but `check.c` still could not include `libavutil/avutil.h`.
- Interpretation: `/project/target/...` is not a usable include path for this
  build-script probe inside the `cross` cargo container. The next attempt should
  stage the FFmpeg dist into the active per-job target directory so the original
  `/target/ffmpeg-minimal/...` path exists again inside `cross`.

Second follow-up fix:

- Commit: `3217abd`
- Change: after restoring/building the shared minimal FFmpeg cache, copy the
  completed `dist` directory into the active per-job Cargo target directory so
  `cross` exposes it at `/target/ffmpeg-minimal/armv7/dist`.
- Run: `27484129889`
- Conclusion: success.
- Workflow wall time: `3m46s`.

Job timings:

| Job | Duration | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------|-------------------------|-------|
| host-dev | 33s | n/a | n/a | all host checks green |
| fast | 1m46s | 1m27s | 3s | GLIBC guard passed |
| device | 2m04s | 1m40s | 3s | GLIBC guard passed |
| fast-video | 3m20s | 2m52s | 1s | GLIBC guard passed; FFmpeg target-dir staging fixed header visibility |
| device-video-full | 3m38s | 3m19s | 0s | GLIBC guard passed; FFmpeg target-dir staging fixed header visibility |

Verdict:

- Reject.
- The per-matrix target-dir layout is now correct, but it is materially slower
  than the accepted safe Docker/cross baseline. The accepted `df4a6c7` repeat was
  `2m28s` wall time with `fast-video` `1m55s` and `device-video-full` `2m17s`;
  this run took `3m46s` wall time with `fast-video` `3m20s` and
  `device-video-full` `3m38s`.
- Likely cause: the per-job target directories reduce cache breadth but lose
  useful sharing and force extra FFmpeg dist staging into target caches. Bun's
  explicit target-dir pattern is useful in their custom Ninja/Cargo graph, but
  in this `cross` setup it made the heaviest matrix jobs slower.
- Next action: restore the broad accepted ARM target cache paths and normal
  `/target/ffmpeg-minimal/...` setup.

Restoration check:

- Commit: `456158d`
- Run: `27484248375`
- Conclusion: success.
- Workflow wall time: `2m28s` by run timestamps (`01:04:45Z` to `01:07:13Z`).

Job timings:

| Job | Duration | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------|-------------------------|-------|
| host-dev | 20s | n/a | n/a | all host checks green |
| fast | 1m37s | 1m14s | 5s | GLIBC guard passed |
| fast-video | 1m39s | 1m13s | 5s | GLIBC guard passed |
| device | 1m56s | 1m40s | 3s | GLIBC guard passed |
| device-video-full | 2m24s | 1m52s | 6s | GLIBC guard passed |

Interpretation:

- The restoration confirms the rejection: broad profile/target caches are much
  faster for the video jobs in this repo than isolated per-matrix target dirs.
  `fast-video` improved from `3m20s` to `1m39s`, and `device-video-full`
  improved from `3m38s` to `2m24s`.

Passive warm repeat:

- Note-only commit: `ed92d32`
- Run: `27484318057`
- Conclusion: success.
- Workflow wall time: `2m23s` by run timestamps (`01:08:06Z` to `01:10:29Z`).

Job timings:

| Job | Duration | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------|-------------------------|-------|
| host-dev | 36s | n/a | n/a | all host checks green |
| fast | 1m30s | 1m11s | 4s | GLIBC guard passed |
| fast-video | 1m37s | 1m17s | 4s | GLIBC guard passed |
| device | 2m18s | 1m59s | 3s | GLIBC guard passed |
| device-video-full | 2m11s | 1m40s | 5s | GLIBC guard passed |

Interpretation:

- This second post-revert sample stays in the accepted safe timing band and
  reinforces the per-matrix target-dir rejection. The fastest video job in the
  broad-cache layout (`1m37s`) is less than half the rejected per-matrix
  `fast-video` runtime (`3m20s`).

Second passive warm repeat:

- Note-only commit: `57b0d5f`
- Run: `27484385730`
- Conclusion: success.
- Workflow wall time: `2m08s` by run timestamps (`01:11:26Z` to `01:13:34Z`).

Job timings:

| Job | Duration | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------|-------------------------|-------|
| host-dev | 28s | n/a | n/a | all host checks green |
| fast | 1m29s | 1m08s | 3s | GLIBC guard passed |
| fast-video | 1m55s | 1m24s | 6s | GLIBC guard passed |
| device | 1m54s | 1m34s | 4s | GLIBC guard passed |
| device-video-full | 2m04s | 1m41s | 8s | GLIBC guard passed |

Interpretation:

- Another green broad-cache sample, and the fastest full workflow since the
  restored Docker/cross path. This gives a good immediate baseline before the
  GitHub Actions Node 24 runtime opt-in.

## Experiment 19: opt GitHub JavaScript actions into Node 24

Change:

- Set `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24=true` at workflow env scope.

Hypothesis:

- This should remove the Node 20 deprecation annotations now appearing on
  `actions/checkout@v4`, `actions/cache@v4`, and `actions/upload-artifact@v4`.
- It should not materially affect Rust build duration because it changes the
  JavaScript runtime for GitHub actions, not the Cargo/cross build graph.

Reason:

- GitHub's own CI annotation says Node 20 actions will be forced to Node 24 by
  default starting `2026-06-16` and suggests setting
  `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24=true` to opt in now. Testing this before
  the default changes keeps the CI path from taking a surprise compatibility hit.

Measure:

- All existing jobs must pass.
- The Node 20 deprecation annotations should disappear or materially change.
- Compare wall time and cache-step durations against the immediate broad-cache
  samples: `27484248375` (`2m28s`), `27484318057` (`2m23s`), and `27484385730`
  (`2m08s`).

Result:

- Commit: `a5fa4fb`
- Run: `27484448508`
- Conclusion: success.
- Workflow wall time: `2m12s` by run timestamps (`01:14:36Z` to `01:16:48Z`).

Job timings:

| Job | Duration | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------|-------------------------|-------|
| host-dev | 27s | n/a | n/a | all host checks green |
| fast | 1m44s | 1m17s | 5s | GLIBC guard passed |
| fast-video | 1m44s | 1m25s | 4s | GLIBC guard passed |
| device | 1m43s | 1m25s | 3s | GLIBC guard passed |
| device-video-full | 2m02s | 1m40s | 3s | GLIBC guard passed |

Annotation result:

- The old warning did not disappear. It changed from "actions are running on
  Node.js 20 and may not work as expected" to "actions target Node.js 20 but are
  being forced to run on Node.js 24".
- This proves the workflow is already exercising the future Node 24 action
  runtime, but GitHub still emits an annotation because the action versions
  declare Node 20.

Verdict:

- Keep as a compatibility hardening change, not a speed win.
- Timing stayed inside the accepted broad-cache band (`2m08s`-`2m28s`), and
  `device-video-full` was the fastest recent full-video job at `2m02s`.
- It does not reduce Rust build work or remove annotations completely, so future
  work should still consider official action updates if/when Node 24-native
  action versions exist.

Passive repeat:

- Note-only commit: `939bbee`
- Run: `27484514546`
- Conclusion: success.
- Workflow wall time: `2m15s` by run timestamps (`01:17:51Z` to `01:20:06Z`).

Job timings:

| Job | Duration | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------|-------------------------|-------|
| host-dev | 47s | n/a | n/a | all host checks green |
| fast | 1m42s | 1m22s | 3s | GLIBC guard passed |
| fast-video | 1m36s | 1m17s | 3s | GLIBC guard passed |
| device | 2m10s | 1m34s | 6s | GLIBC guard passed; upload step took 7s |
| device-video-full | 2m06s | 1m49s | 3s | GLIBC guard passed |

Interpretation:

- A second Node 24-runtime sample remains green and inside the accepted timing
  band. Keep the opt-in unless a later action-version experiment makes it
  unnecessary.

Second passive repeat:

- Note-only commit: `939bbee`
- Run: `27484514546`
- Conclusion: success.
- Workflow wall time: `2m15s` by run timestamps (`01:17:51Z` to `01:20:06Z`).

Job timings:

| Job | Duration | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------|-------------------------|-------|
| host-dev | 47s | n/a | n/a | all host checks green |
| fast | 1m42s | 1m22s | 3s | GLIBC guard passed |
| fast-video | 1m36s | 1m17s | 3s | GLIBC guard passed |
| device | 2m10s | 1m34s | 6s | GLIBC guard passed; upload step took 7s |
| device-video-full | 2m06s | 1m49s | 3s | GLIBC guard passed |

Interpretation:

- The second Node 24-runtime repeat remains green and inside the same broad-cache
  timing band. The forced-Node-24 annotation remains present.

## Experiment 20: disable Cargo HTML timing reports in CI

Change:

- Teach `build-arm.sh` to include `cargo --timings` only when
  `MISTER_CARGO_TIMINGS` is not `0`.
- Set `MISTER_CARGO_TIMINGS=0` in the GitHub Actions workflow.
- Keep local/default `build-arm.sh` behavior unchanged, so manual builds still
  emit Cargo timing HTML reports.

Hypothesis:

- CI does not upload or consume the Cargo HTML timing report. Disabling report
  generation in CI may shave a small amount of overhead from every ARM build
  while retaining GitHub job/step timings and log-based timing data.

Risk:

- The win may be pure noise.
- We lose per-crate Cargo timing HTML from CI logs/runners, though those reports
  were not persisted as artifacts.

Measure:

- All existing jobs must pass.
- Compare `Build ARM binary` and workflow wall time against recent Node 24
  broad-cache samples:
  - `27484448508`: `2m12s` wall; build steps `1m17s`, `1m25s`, `1m25s`, `1m40s`
  - `27484514546`: `2m15s` wall; build steps `1m22s`, `1m17s`, `1m34s`, `1m49s`

Seed result:

- Commit: `6d1c340`
- Run: `27484621508`
- Conclusion: success.
- Workflow wall time: `3m59s` by run timestamps (`01:23:20Z` to `01:27:19Z`).
- This is a seed run because changing `build-arm.sh` changed the ARM target
  cache key.

Job timings:

| Job | Duration | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------|-------------------------|-------|
| host-dev | 39s | n/a | n/a | all host checks green |
| fast | 3m29s | 2m59s | 5s | GLIBC guard passed |
| fast-video | 3m29s | 2m56s | 4s | GLIBC guard passed |
| device | 3m56s | 3m21s | 8s | GLIBC guard passed |
| device-video-full | 3m43s | 3m14s | 4s | GLIBC guard passed |

Interpretation:

- Not a verdict. The new target-cache key forced a cold/seed run, similar to
  earlier Dockerfile/build-script experiments.
- A warm follow-up is required before deciding whether disabling CI Cargo timing
  reports helps or hurts.

Warm result:

- Note-only commit: `376d243`
- Run: `27484723664`
- Conclusion: success.
- Workflow wall time: `2m11s` by run timestamps (`01:28:29Z` to `01:30:40Z`).

Job timings:

| Job | Duration | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------|-------------------------|-------|
| host-dev | 49s | n/a | n/a | all host checks green |
| fast | 1m17s | 56s | 4s | GLIBC guard passed |
| fast-video | 1m44s | 1m19s | 4s | GLIBC guard passed |
| device | 1m45s | 1m29s | 3s | GLIBC guard passed |
| device-video-full | 2m07s | 1m37s | 6s | GLIBC guard passed |

Interpretation:

- Provisional keep, pending one passive repeat.
- This is inside the best accepted broad-cache band and improves most build-step
  timings versus the immediate Node 24 samples:
  - `fast`: `56s` versus `1m17s` / `1m22s`
  - `device`: `1m29s` versus `1m25s` / `1m34s`
  - `device-video-full`: `1m37s` versus `1m40s` / `1m49s`
- The win is not uniform (`fast-video` was `1m19s`, between the recent `1m25s`
  and `1m17s` samples), so run-to-run noise is still large. A passive repeat is
  needed before treating this as accepted.

Passive repeat:

- Note-only commit: `0ee3f0d`
- Run: `27484792179`
- Conclusion: success.
- Workflow wall time: `2m05s` by run timestamps (`01:31:44Z` to `01:33:49Z`).

Job timings:

| Job | Duration | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------|-------------------------|-------|
| host-dev | 48s | n/a | n/a | all host checks green |
| fast | 1m36s | 1m12s | 5s | GLIBC guard passed |
| fast-video | 1m37s | 1m19s | 4s | GLIBC guard passed |
| device | 1m56s | 1m37s | 4s | GLIBC guard passed |
| device-video-full | 2m01s | 1m41s | 4s | GLIBC guard passed |

Verdict:

- Accept.
- The two warm runs after disabling CI Cargo timing HTML reports were `2m11s`
  and `2m05s`, both at the fast end of the accepted broad-cache range.
- This keeps all matrix jobs, tests, artifact uploads, and GLIBC checks intact.
  The tradeoff is losing non-persisted Cargo HTML timing reports in CI; GitHub
  job/step timings and logs remain available, and local `build-arm.sh` still
  emits Cargo timings by default.

Extra passive sample after acceptance:

- Note-only commit: `769db8b`
- Run: `27484876037`
- Conclusion: success.
- Workflow wall time: `2m10s` by run timestamps (`01:35:51Z` to `01:38:01Z`).

Job timings:

| Job | Duration | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------|-------------------------|-------|
| host-dev | 46s | n/a | n/a | all host checks green |
| fast | 1m34s | 1m12s | 8s | GLIBC guard passed |
| fast-video | 1m41s | 1m18s | 4s | GLIBC guard passed |
| device | 2m05s | 1m38s | 5s | GLIBC guard passed |
| device-video-full | 1m47s | 1m21s | 6s | GLIBC guard passed |

Interpretation:

- Confirms the accepted Cargo-timings change remains in the same warm band:
  three warm/passive samples are now `2m11s`, `2m05s`, and `2m10s`.

## Experiment 21: remove redundant ARM target install command

Change:

- Remove the explicit `rustup target add armv7-unknown-linux-gnueabihf` from the
  ARM matrix setup step.
- Keep `rustup show` running in `magik-gui`, where `rust-toolchain.toml` already
  declares the ARM target under `targets`.

Hypothesis:

- `rustup show` should resolve the pinned stable toolchain and declared target
  from `rust-toolchain.toml`, making the separate `rustup target add` a duplicate
  check/install in every ARM matrix job.
- If correct, this saves a few seconds of setup per ARM job without changing any
  build, test, upload, cache, or GLIBC-check behavior.

Risk:

- If `rustup show` does not install/sync the declared target on GitHub runners,
  `cross` or Cargo may fail with a missing `std`/target error. That would be a
  clean reject and immediate revert.

Measure:

- All existing jobs must pass.
- Compare the ARM `Install Rust toolchain` setup step and total workflow wall
  time against the accepted warm band.

Seed result:

- Commit: `ca46d73`
- Run: `27484939757`
- Conclusion: success.
- Workflow wall time: `2m15s` by run timestamps (`01:39:09Z` to `01:41:24Z`).

Job timings:

| Job | Duration | Install Rust toolchain | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------------|------------------|-------------------------|-------|
| host-dev | 26s | 9s | n/a | n/a | all host checks green |
| fast | 1m28s | 3s | 1m13s | 3s | GLIBC guard passed |
| fast-video | 1m38s | 2s | 1m18s | 4s | GLIBC guard passed |
| device | 1m59s | 2s | 1m34s | 6s | GLIBC guard passed |
| device-video-full | 2m10s | 2s | 1m42s | 5s | GLIBC guard passed |

Interpretation:

- Provisional keep pending passive repeat.
- Correctness is confirmed: `rustup show` in `magik-gui` resolved enough
  toolchain/target state for all four ARM builds, so the explicit
  `rustup target add` was not required for this workflow.
- The timing win is tiny and mostly hidden by normal build-step variance. ARM
  setup was `2s`-`3s`, compared with `2s`-`3s` before the change and at most
  about one second better on the `device` jobs.
- Wall time (`2m15s`) is slower than the previous passive sample (`2m10s`), but
  the slower `device-video-full` build step (`1m42s`) explains most of that.
  Run one note-only repeat before accepting or reverting.

Passive repeat:

- Note-only commit: `1c18ed0`
- Run: `27485003725`
- Conclusion: success.
- Workflow wall time: `2m20s` by run timestamps (`01:42:26Z` to `01:44:46Z`).

Job timings:

| Job | Duration | Install Rust toolchain | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------------|------------------|-------------------------|-------|
| host-dev | 51s | 13s | n/a | n/a | all host checks green |
| fast | 1m33s | 4s | 1m14s | 4s | GLIBC guard passed |
| fast-video | 1m55s | 3s | 1m20s | 6s | GLIBC guard passed; FFmpeg cache took 7s |
| device | 1m56s | 3s | 1m30s | 5s | GLIBC guard passed |
| device-video-full | 2m15s | 4s | 1m51s | 4s | GLIBC guard passed |

Verdict:

- Reject for timing, despite being functionally safe.
- Removing the explicit `rustup target add` did not produce a meaningful setup
  win. ARM install steps remained `2s`-`4s`, which overlaps the previous
  accepted-state samples.
- The two runs after the change were `2m15s` and `2m20s`, slower than the recent
  accepted warm band (`2m05s`-`2m11s`). Build-step variance explains much of the
  slowdown, but the experiment still failed to demonstrate a CI-time reduction.
- Revert the workflow line and keep the timing evidence here.

Restoration run after revert:

- Commit: `2f7533d`
- Run: `27485078823`
- Conclusion: success.
- Workflow wall time: `2m24s` by run timestamps (`01:46:16Z` to `01:48:40Z`).

Job timings:

| Job | Duration | Install Rust toolchain | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------------|------------------|-------------------------|-------|
| host-dev | 20s | 2s | n/a | n/a | all host checks green |
| fast | 1m38s | 3s | 1m10s | 9s | GLIBC guard passed |
| fast-video | 1m53s | 5s | 1m25s | 5s | GLIBC guard passed |
| device | 2m07s | 5s | 1m36s | 5s | GLIBC guard passed |
| device-video-full | 2m14s | 4s | 1m51s | 4s | GLIBC guard passed |

Interpretation:

- Restoration is green, but still in a slower warm-cache patch of runner
  variance. This supports the rejection: toggling the target-add line did not
  control the wall-clock behavior.

## Experiment 22: upgrade GitHub actions to native Node 24 releases

Sources checked before changing:

- `actions/checkout` releases show `v6.0.3` as latest, and the v6 release notes
  mention Node.js 24 support/details.
- `actions/cache` releases show `v5.0.5` as latest; the v5 notes say
  `actions/cache@v5` runs on Node.js 24 and requires Actions Runner `2.327.1`.
- `actions/upload-artifact` releases show `v7.0.0`; the v6 notes say
  `upload-artifact@v6` runs on Node.js 24, and v7 adds direct single-file uploads
  behind a new option while keeping the normal archive path by default.

Change:

- Use `actions/checkout@v6`.
- Use `actions/cache@v5`.
- Use `actions/upload-artifact@v7`.
- Remove `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24` because these action majors should
  select Node 24 natively.

Hypothesis:

- Native Node 24 actions should remove the forced-runtime annotations.
- The newer action bundles may slightly reduce setup/cache/upload overhead versus
  forcing Node 24 on older major versions. The likely effect is small, but this
  is a clean compatibility/timing experiment on GitHub-hosted runners.

Risk:

- New major versions may change action behavior. The runner-version requirement
  should be satisfied on GitHub-hosted runners, but the matrix will prove it.
- `upload-artifact@v7` defaults must remain compatible with the existing binary
  and `binary-size.tsv` uploads; do not enable the new direct-upload mode in this
  experiment.

Measure:

- All existing jobs must pass.
- The forced Node 24 annotations should disappear or reduce.
- Compare checkout/cache/upload step timings and total workflow wall time against
  the accepted warm band.

Seed result:

- Commit: `8811716`
- Run: `27485158700`
- Conclusion: success.
- Workflow wall time: `2m31s` by run timestamps (`01:50:13Z` to `01:52:44Z`).
- Watch output showed no forced Node 24 annotations.

Job timings:

| Job | Duration | Checkout | Cache cargo registry | Cache ARM build outputs | Upload ARM binary | Upload size history | Build ARM binary | Notes |
|-----|----------|----------|----------------------|-------------------------|-------------------|---------------------|------------------|-------|
| host-dev | 29s | 1s | 6s | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m22s | 0s | 3s | 3s | 1s | 1s | 1m04s | GLIBC guard passed |
| fast-video | 1m45s | 1s | 4s | 9s | 1s | 1s | 1m18s | GLIBC guard passed |
| device | 2m04s | 1s | 13s | 5s | 1s | 2s | 1m33s | GLIBC guard passed; cargo registry cache was slow |
| device-video-full | 2m20s | 2s | 3s | 3s | 1s | 1s | 1m58s | GLIBC guard passed |

Interpretation:

- Provisional keep pending passive repeat.
- Compatibility is confirmed across checkout, cache restore/save, artifact
  upload, and all Rust/GLIBC checks.
- The forced Node 24 annotations disappeared, which is useful maintenance signal.
- Timing is not yet a win. Wall time (`2m31s`) is slower than the accepted warm
  band, mainly because `device-video-full` spent `1m58s` in the build step and
  one `device` cargo registry cache restore took `13s`. Run a note-only repeat
  before accepting or rejecting.

Passive repeat:

- Note-only commit: `158a4f6`
- Run: `27485230333`
- Conclusion: success.
- Workflow wall time: `2m22s` by run timestamps (`01:53:51Z` to `01:56:13Z`).
- Watch output again showed no forced Node 24 annotations.

Job timings:

| Job | Duration | Checkout | Cache cargo registry | Cache ARM build outputs | Upload ARM binary | Upload size history | Build ARM binary | Notes |
|-----|----------|----------|----------------------|-------------------------|-------------------|---------------------|------------------|-------|
| host-dev | 35s | 2s | 6s | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m24s | 1s | 3s | 3s | 1s | 1s | 1m07s | GLIBC guard passed |
| fast-video | 1m45s | 1s | 4s | 4s | 1s | 1s | 1m18s | GLIBC guard passed; FFmpeg cache took 7s |
| device | 1m54s | 1s | 4s | 3s | 0s | 1s | 1m37s | GLIBC guard passed |
| device-video-full | 2m17s | 1s | 6s | 6s | 2s | 1s | 1m47s | GLIBC guard passed |

Verdict:

- Accept as compatibility cleanup, not as a measured timing win.
- The new action majors are compatible with the full workflow: host tests, ARM
  builds, GLIBC checks, binary uploads, and size artifact uploads all passed
  twice.
- The forced Node 24 annotations disappeared in both watch outputs, so the
  workflow no longer relies on `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24`.
- Timing is neutral-to-noisy: the two runs were `2m31s` and `2m22s`, not better
  than the best accepted Cargo-timings warm samples (`2m05s`-`2m11s`) but inside
  the broader warm-cache spread observed during the target-install rejection
  (`2m15s`-`2m24s`). Keep for action-runtime hygiene, not runtime reduction.

Extra passive sample after acceptance:

- Note-only commit: `edb235e`
- Run: `27485296605`
- Conclusion: success.
- Workflow wall time: `2m12s` by run timestamps (`01:57:16Z` to `01:59:28Z`).
- Watch output again showed no forced Node 24 annotations.

Job timings:

| Job | Duration | Checkout | Cache cargo registry | Cache ARM build outputs | Upload ARM binary | Upload size history | Build ARM binary | Notes |
|-----|----------|----------|----------------------|-------------------------|-------------------|---------------------|------------------|-------|
| host-dev | 33s | 2s | 9s | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m30s | 1s | 3s | 3s | 2s | 1s | 1m12s | GLIBC guard passed |
| fast-video | 1m34s | 1s | 4s | 4s | 3s | 1s | 1m14s | GLIBC guard passed |
| device | 2m01s | 1s | 3s | 7s | 1s | 0s | 1m40s | GLIBC guard passed |
| device-video-full | 2m01s | 0s | 4s | 4s | 1s | 1s | 1m42s | GLIBC guard passed |

Interpretation:

- Strengthens the keep decision. Native action majors remain green, annotation-free,
  and this passive sample returned to the accepted warm band.

Second passive sample after acceptance:

- Note-only commit: `6f09ad6`
- Run: `27485370058`
- Conclusion: success.
- Workflow wall time: `2m11s` by run timestamps (`02:00:47Z` to `02:02:58Z`).

Job timings:

| Job | Duration | Checkout | Cache cargo registry | Cache ARM build outputs | Upload ARM binary | Upload size history | Build ARM binary | Notes |
|-----|----------|----------|----------------------|-------------------------|-------------------|---------------------|------------------|-------|
| host-dev | 36s | 1s | 7s | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m23s | 1s | 3s | 3s | 1s | 1s | 1m07s | GLIBC guard passed |
| fast-video | 1m43s | 0s | 3s | 3s | 1s | 1s | 1m19s | GLIBC guard passed; FFmpeg cache took 6s |
| device | 2m06s | 1s | 9s | 4s | 2s | 1s | 1m38s | GLIBC guard passed |
| device-video-full | 2m06s | 1s | 4s | 9s | 1s | 1s | 1m40s | GLIBC guard passed |

Interpretation:

- Confirms the native-action accepted state can still hit the best warm band:
  the last two passive samples were `2m12s` and `2m11s`.

## Experiment 23: remove the ARM rustup setup step entirely

Change:

- Remove the ARM matrix `Install Rust toolchain` step (`rustup show` plus
  `rustup target add armv7-unknown-linux-gnueabihf`).
- Keep the host-dev toolchain step unchanged.
- Keep every ARM build, GLIBC check, cache, and artifact upload.

Hypothesis:

- The ARM jobs may not need a separate host-side rustup setup step because the
  actual build runs through `cross` from inside `magik-gui`, where
  `rust-toolchain.toml` already declares the target.
- If `cross`/Cargo resolves the same toolchain state lazily, the work may simply
  move into `Build ARM binary`. If the target is truly unnecessary host-side, this
  could save roughly `2s`-`5s` per ARM job.

Risk:

- The jobs may fail with a missing target/toolchain error. That is a clean reject.
- The setup cost may just move from the explicit install step into the build
  step, producing no wall-clock win.

Measure:

- All existing jobs must pass.
- Compare total wall time and `Build ARM binary` durations against the native
  action accepted samples:
  - `27485296605`: `2m12s`
  - `27485370058`: `2m11s`

Seed result:

- Commit: `6f445d3`
- Run: `27485449866`
- Conclusion: success.
- Workflow wall time: `2m09s` by run timestamps (`02:04:19Z` to `02:06:28Z`).

Job timings:

| Job | Duration | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------|-------------------------|-------|
| host-dev | 35s | n/a | n/a | all host checks green |
| fast | 1m48s | 1m17s | 9s | GLIBC guard passed; cargo registry cache took 10s |
| fast-video | 1m42s | 1m20s | 7s | GLIBC guard passed |
| device | 1m57s | 1m41s | 3s | GLIBC guard passed |
| device-video-full | 2m05s | 1m44s | 4s | GLIBC guard passed |

Interpretation:

- Provisional keep pending passive repeat.
- The workflow remained correct with no explicit ARM rustup setup step.
- Wall time (`2m09s`) is a tiny improvement over the immediately preceding
  native-action baseline (`2m11s`), but most of the saved setup time appears to
  move into `Build ARM binary`: the previous baseline build steps were `1m07s`,
  `1m19s`, `1m38s`, and `1m40s`, versus `1m17s`, `1m20s`, `1m41s`, and `1m44s`
  here.
- Run one note-only repeat before treating this as accepted.

Passive repeat:

- Note-only commit: `b59f3d9`
- Run: `27485527965`
- Conclusion: success.
- Workflow wall time: `2m21s` by run timestamps (`02:07:35Z` to `02:09:56Z`).

Job timings:

| Job | Duration | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------|-------------------------|-------|
| host-dev | 37s | n/a | n/a | all host checks green |
| fast | 1m40s | 1m25s | 3s | GLIBC guard passed |
| fast-video | 1m52s | 1m31s | 3s | GLIBC guard passed; size upload took 6s |
| device | 1m38s | 1m19s | 4s | GLIBC guard passed |
| device-video-full | 2m18s | 1m59s | 3s | GLIBC guard passed |

Verdict:

- Reject.
- Removing the explicit ARM rustup setup step is functionally safe, but it did
  not produce a stable timing win. The repeat regressed to `2m21s`, outside the
  immediately preceding native-action accepted samples (`2m12s`, `2m11s`).
- The setup work mostly moved into `Build ARM binary`, making the build-step
  timings less predictable. Restore the explicit setup step for clearer logs and
  steadier measurement boundaries.

Restoration run after revert:

- Commit: `bcd70f6`
- Run: `27485616310`
- Conclusion: success.
- Workflow wall time: `2m15s` by run timestamps (`02:12:00Z` to `02:14:15Z`).

Job timings:

| Job | Duration | Install Rust toolchain | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------------|------------------|-------------------------|-------|
| host-dev | 20s | 2s | n/a | n/a | all host checks green |
| fast | 1m43s | 5s | 1m22s | 3s | GLIBC guard passed |
| fast-video | 1m44s | 3s | 1m22s | 3s | GLIBC guard passed; FFmpeg cache took 6s |
| device | 2m10s | 3s | 1m54s | 2s | GLIBC guard passed |
| device-video-full | 2m11s | 2s | 1m41s | 4s | GLIBC guard passed; FFmpeg cache took 11s |

Interpretation:

- Restoration is green. The explicit setup step is back, but this sample still
  shows normal runner/build variance rather than a clean timing control.

## Experiment 24: Cargo offline mode on exact registry cache hits

Change:

- Give the ARM `Cache cargo registry` step an id.
- Pass `MISTER_CARGO_OFFLINE=1` to `build-arm.sh` only when that cache step
  reports an exact hit.
- Teach `build-arm.sh` to add `cargo --offline` in that case.

Hypothesis:

- Warm ARM builds with an exact registry cache hit may avoid registry/index
  network checks and shave a little time from `Build ARM binary`.
- Cold builds or changed-lockfile builds stay online because cache misses set
  `MISTER_CARGO_OFFLINE=0`.

Risk:

- An exact cache hit can still be incomplete if the cache was created before
  all needed sources were present, causing an offline Cargo failure.
- Touching `build-arm.sh` changes the ARM target-cache key, so the first run is
  expected to be a cold/seed run. The warm repeat is the real measurement.

Measure:

- All existing jobs must pass.
- On the seed run, confirm offline mode is enabled only when expected.
- Use the warm follow-up to compare wall time and `Build ARM binary` durations
  against the accepted native-action samples.

Seed result:

- Commit: `cce1d4d`
- Run: `27485703726`
- Conclusion: success.
- Workflow wall time: `2m14s` by run timestamps (`02:16:20Z` to `02:18:34Z`).
- Log check: ARM build logs include `MISTER_CARGO_OFFLINE: 1` and
  `==> cargo offline mode enabled (exact registry cache hit)` on exact cargo
  registry cache hits.

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|------------------|-------------------------|-------|
| host-dev | 29s | 4s | 7s | n/a | n/a | all host checks green |
| fast | 1m47s | 5s | 3s | 1m16s | 3s | GLIBC guard passed; target post-cache took 8s |
| fast-video | 1m47s | 3s | 4s | 1m19s | 4s | GLIBC guard passed; FFmpeg cache took 6s |
| device | 2m05s | 3s | 5s | 1m36s | 5s | GLIBC guard passed |
| device-video-full | 2m11s | 2s | 4s | 1m48s | 4s | GLIBC guard passed |

Interpretation:

- Provisional keep pending passive repeat.
- The seed run is already in the accepted timing band (`2m14s`) despite
  touching `build-arm.sh`, which changes the target-cache key material.
- The fastest ARM profile improved relative to the immediate restoration run
  (`fast` build `1m16s` here versus `1m22s`), while the slowest
  `device-video-full` job remained the wall-clock limiter at `2m11s`.
- Need one note-only warm repeat before accepting or rejecting. If the repeat is
  not clearly better than the `2m05s`-`2m12s` native-action range, reject this
  as extra CI complexity with no durable win.

Warm repeat:

- Note-only commit: `765bcbe`
- Run: `27485796967`
- Conclusion: success.
- Workflow wall time: `2m19s` by run timestamps (`02:21:00Z` to `02:23:19Z`).
- Log check: all four ARM jobs restored the exact cargo registry cache key and
  ran with `MISTER_CARGO_OFFLINE: 1`; each printed
  `==> cargo offline mode enabled (exact registry cache hit)`.

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|------------------|-------------------------|-------|
| host-dev | 51s | 9s | 9s | n/a | n/a | all host checks green |
| fast | 1m25s | 3s | 4s | 1m06s | 3s | GLIBC guard passed |
| fast-video | 1m30s | 5s | 3s | 1m04s | 4s | GLIBC guard passed |
| device | 1m52s | 2s | 4s | 1m34s | 4s | GLIBC guard passed |
| device-video-full | 2m14s | 3s | 6s | 1m44s | 6s | GLIBC guard passed |

Verdict:

- Reject.
- `cargo --offline` on exact registry cache hits is functionally safe here and
  improved the two fast-profile build steps in this sample, but the total
  workflow time regressed to `2m19s` and did not beat the recent accepted samples
  (`2m05s`, `2m10s`, `2m11s`, `2m12s`).
- The win is not durable enough to justify extra workflow/build-script
  branching. Revert the offline-mode plumbing and keep the registry cache as a
  normal online Cargo cache.

Restoration run after revert:

- Commit: `24bbe47`
- Run: `27485877143`
- Conclusion: success.
- Workflow wall time: `2m07s` by run timestamps (`02:25:02Z` to `02:27:09Z`).

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|------------------|-------------------------|-------|
| host-dev | 29s | 2s | 7s | n/a | n/a | all host checks green |
| fast | 1m33s | 3s | 4s | 1m12s | 4s | GLIBC guard passed |
| fast-video | 1m57s | 3s | 4s | 1m30s | 5s | GLIBC guard passed |
| device | 1m57s | 3s | 4s | 1m35s | 3s | GLIBC guard passed |
| device-video-full | 2m03s | 2s | 8s | 1m39s | 3s | GLIBC guard passed |

Interpretation:

- The revert restored the workflow to the accepted timing band (`2m07s` wall).
- This supports the rejection: the offline-mode sample had a few faster
  individual builds but did not improve the workflow tail.

Passive restored baseline:

- Note-only commit: `9d240be`
- Run: `27485935438`
- Conclusion: success.
- Workflow wall time: `2m09s` by run timestamps (`02:27:56Z` to `02:30:05Z`).

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|------------------|-------------------------|-------|
| host-dev | 31s | 3s | 8s | n/a | n/a | all host checks green |
| fast | 1m33s | 3s | 5s | 1m07s | 5s | GLIBC guard passed |
| fast-video | 1m47s | 3s | 5s | 1m19s | 4s | GLIBC guard passed |
| device | 1m58s | 4s | 3s | 1m42s | 2s | GLIBC guard passed |
| device-video-full | 2m05s | 3s | 4s | 1m38s | 5s | GLIBC guard passed |

Interpretation:

- Restored path is stable around `2m07s`-`2m09s` after rejecting offline mode.
- The workflow tail remains `device-video-full`; host-dev is not the limiting
  job.

## Experiment 25: explicit Cargo bin target for ARM builds

Change:

- When `build-arm.sh` is building the normal MiSTer frontend, add
  `--bin mister-magik-fb` to `cross build`.
- Preserve the existing `--bin preview-archive-bench` path for the separate
  preview archive benchmark helper.

Hypothesis:

- The package has two binary targets, but the CI artifact always needs
  `mister-magik-fb`. The other bin requires a separate feature and should not be
  built in normal CI, but explicitly naming the target may reduce Cargo target
  selection or metadata work slightly.

Risk:

- Low functional risk: the same production binary is built and uploaded.
- The first run will be a seed/cold-ish run because `build-arm.sh` participates
  in the ARM target-cache key.

Measure:

- All existing jobs must pass and the GLIBC guard must still pass.
- Warm repeat must beat the restored baseline range (`2m07s`-`2m09s`) or the
  change is not worth keeping.

Seed result:

- Commit: `a2b983b`
- Run: `27486010393`
- Conclusion: success.
- Workflow wall time: `2m34s` by run timestamps (`02:31:32Z` to `02:34:06Z`).

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|------------------|-------------------------|-------|
| host-dev | 31s | 2s | 8s | n/a | n/a | all host checks green |
| fast | 1m43s | 4s | 3s | 1m16s | 3s | GLIBC guard passed; size upload took 5s |
| fast-video | 1m51s | 4s | 4s | 1m20s | 4s | GLIBC guard passed; target post-cache took 5s |
| device | 1m48s | 3s | 3s | 1m29s | 3s | GLIBC guard passed; target post-cache took 3s |
| device-video-full | 2m29s | 3s | 5s | 1m52s | 6s | GLIBC guard passed; target post-cache took 5s |

Interpretation:

- Seed is slow, as expected after touching `build-arm.sh` and changing the ARM
  target-cache key material.
- Functional correctness is fine. Run one note-only warm repeat before judging.

Warm repeat:

- Note-only commit: `91df44e`
- Run: `27486083582`
- Conclusion: success.
- Workflow wall time: `2m17s` by run timestamps (`02:35:12Z` to `02:37:29Z`).

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|------------------|-------------------------|-------|
| host-dev | 23s | 3s | 5s | n/a | n/a | all host checks green |
| fast | 1m53s | 3s | 5s | 1m28s | 4s | GLIBC guard passed |
| fast-video | 1m49s | 3s | 6s | 1m19s | 6s | GLIBC guard passed |
| device | 1m49s | 3s | 4s | 1m32s | 3s | GLIBC guard passed |
| device-video-full | 2m13s | 4s | 10s | 1m40s | 5s | GLIBC guard passed |

Verdict:

- Reject.
- Explicit `--bin mister-magik-fb` is functionally safe but did not improve the
  warm run. Workflow wall time was `2m17s`, slower than the restored baseline
  range (`2m07s`-`2m09s`).
- Remove the extra `--bin` argument and return to Cargo's default target
  selection.

Restoration run after revert:

- Commit: `c8b4050`
- Run: `27486146792`
- Conclusion: success.
- Workflow wall time: `2m19s` by run timestamps (`02:38:31Z` to `02:40:50Z`).

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | Build ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|------------------|-------------------------|-------|
| host-dev | 33s | 10s | 6s | n/a | n/a | all host checks green |
| fast | 1m26s | 6s | 3s | 1m05s | 3s | GLIBC guard passed |
| fast-video | 1m42s | 3s | 3s | 1m17s | 3s | GLIBC guard passed |
| device | 1m52s | 3s | 5s | 1m29s | 4s | GLIBC guard passed |
| device-video-full | 2m08s | 4s | 4s | 1m47s | 3s | GLIBC guard passed |

Interpretation:

- The explicit-bin revert returned the ARM job durations to the normal range.
- Overall wall time is noisy because `device-video-full` started about ten
  seconds after the run was created, but the job duration (`2m08s`) is back in
  the accepted band.

## Experiment 26: stop uploading size-history artifact

Change:

- Remove the `Upload size history` artifact step from each ARM matrix job.
- Keep `record-binary-size.sh` in `build-arm.sh`, so binary size is still
  measured during every build.
- Keep every build, GLIBC check, and main `mister-magik-fb-*` binary artifact.

Hypothesis:

- Uploading `build/binary-size.tsv` as a separate artifact costs roughly
  `0s`-`2s` per job and sometimes appears on the workflow tail.
- Removing only this nonessential artifact upload may shave a small amount of
  wall-clock time without avoiding builds or tests.

Risk:

- We lose the separate downloadable binary-size artifact from CI runs.
- Binary size remains in the build logs/generated file, but not as its own
  artifact.

Measure:

- All existing jobs must pass.
- Main ARM binary artifacts must still upload.
- Compare wall time and tail job duration against the restored baseline band.

First result:

- Commit: `5a6a674`
- Run: `27486220234`
- Conclusion: success.
- Workflow wall time: `2m28s` by run timestamps (`02:42:19Z` to `02:44:47Z`).
- Verification: every ARM job still ran `Upload ARM binary`; the removed
  `Upload size history` step is absent from the job graph.

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | Build ARM binary | Upload ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|------------------|-------------------|-------------------------|-------|
| host-dev | 39s | 11s | 8s | n/a | n/a | n/a | all host checks green |
| fast | 1m37s | 3s | 4s | 1m18s | 1s | 4s | GLIBC guard passed |
| fast-video | 2m00s | 6s | 4s | 1m31s | 2s | 4s | GLIBC guard passed |
| device | 1m52s | 3s | 3s | 1m32s | 0s | 3s | GLIBC guard passed |
| device-video-full | 2m23s | 3s | 4s | 2m00s | 2s | 4s | GLIBC guard passed |

Interpretation:

- First sample is not a keep: wall time and `device-video-full` both regressed.
- The regression is mostly build-time variance, not artifact overhead, so run
  one note-only repeat before deciding.

Repeat:

- Note-only commit: `1e7082c`
- Run: `27486286846`
- Conclusion: success.
- Workflow wall time: `2m14s` by run timestamps (`02:45:54Z` to `02:48:08Z`).
- Verification: every ARM job still ran `Upload ARM binary`; `Upload size
  history` remained absent.

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | Build ARM binary | Upload ARM binary | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|------------------|-------------------|-------------------------|-------|
| host-dev | 26s | 3s | 5s | n/a | n/a | n/a | all host checks green |
| fast | 1m35s | 4s | 4s | 1m18s | 1s | 3s | GLIBC guard passed |
| fast-video | 1m31s | 3s | 3s | 1m14s | 1s | 4s | GLIBC guard passed |
| device | 2m00s | 3s | 4s | 1m33s | 2s | 9s | GLIBC guard passed |
| device-video-full | 2m10s | 2s | 6s | 1m40s | 2s | 6s | GLIBC guard passed |

Verdict:

- Reject.
- Removing the size-history artifact is functionally safe for builds/tests and
  leaves main binary artifacts intact, but the timing win is not clear. Samples
  were `2m28s` and `2m14s`, versus a restored baseline around `2m07s`-`2m09s`
  by job duration.
- Restore the artifact because the small possible savings do not justify losing
  downloadable size history from CI.

Restoration run after revert:

- Commit: `ece6758`
- Run: `27486348643`
- Conclusion: success.
- Workflow wall time: `2m20s` by run timestamps (`02:49:17Z` to `02:51:37Z`).

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | Build ARM binary | Upload ARM binary | Upload size history | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|------------------|-------------------|---------------------|-------------------------|-------|
| host-dev | 22s | 2s | 4s | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m30s | 3s | 3s | 1m11s | 1s | 1s | 3s | GLIBC guard passed |
| fast-video | 1m47s | 3s | 3s | 1m20s | 1s | 0s | 3s | GLIBC guard passed; cross cache took 11s |
| device | 2m10s | 4s | 9s | 1m38s | 1s | 1s | 4s | GLIBC guard passed; cross cache took 6s |
| device-video-full | 2m15s | 3s | 6s | 1m44s | 3s | 1s | 6s | GLIBC guard passed |

Interpretation:

- Restored artifact step is green.
- The size-history upload itself costs only `0s`-`1s` in this sample; the
  larger variance is in cache restore and build time. Artifact removal is not a
  meaningful lever.

Passive accepted-state sample:

- Note-only commit: `2e69496`
- Run: `27486434357`
- Conclusion: success.
- Workflow wall time: `2m23s` by run timestamps (`02:53:49Z` to `02:56:12Z`).

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|------------------|----------------------------|-------------------|---------------------|-------------------------|-------|
| host-dev | 43s | 11s | 6s | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m39s | 3s | 5s | 1m15s | 1s | 1s | 1s | 4s | GLIBC guard passed |
| fast-video | 1m47s | 4s | 4s | 1m21s | 0s | 1s | 1s | 4s | GLIBC guard passed; FFmpeg cache took 6s |
| device | 2m12s | 5s | 5s | 1m45s | 1s | 1s | 2s | 5s | GLIBC guard passed |
| device-video-full | 2m17s | 3s | 6s | 1m46s | 0s | 3s | 1s | 7s | GLIBC guard passed |

Interpretation:

- Accepted state remains green but noisy, with `device-video-full` still the
  tail.
- The separate shared-library check is tiny (`0s`-`1s`) but creates another
  Actions step boundary per ARM job.

## Experiment 27: fold ARM shared-library check into build step

Change:

- Replace the separate `Check ARM shared libraries` step with a second command
  in the `Build ARM binary` step.
- Keep the exact same `check-arm-shared-libs.sh` invocation, all ARM builds,
  host checks, and artifact uploads.

Hypothesis:

- Removing one Actions step boundary per ARM matrix job may save a very small
  amount of job overhead.
- The shared-library/GLIBC guard still runs before artifacts upload.

Risk:

- Logs become slightly less segmented: build and shared-library check output
  share a step.
- If the build fails, the check naturally will not run, same as before.

Measure:

- All jobs must pass.
- Logs must show `==> max GLIBC symbol version: GLIBC_2.31` for ARM jobs.
- Compare wall time and ARM job durations against the accepted-state samples.

First result:

- Commit: `3c541fa`
- Run: `27486503645`
- Conclusion: success.
- Workflow wall time: `2m08s` by run timestamps (`02:57:26Z` to `02:59:34Z`).
- Log check: all four ARM jobs printed `==> max GLIBC symbol version:
  GLIBC_2.31` inside `Build ARM binary` before `Upload ARM binary`.

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | FFmpeg cache | Build ARM binary | Upload ARM binary | Upload size history | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|--------------|------------------|-------------------|---------------------|-------------------------|-------|
| host-dev | 51s | 10s | 10s | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache 7s |
| fast | 1m28s | 2s | 4s | n/a | 1m07s | 1s | 0s | 8s | GLIBC guard passed inside build step |
| fast-video | 1m23s | 2s | 3s | 2s | 1m02s | 2s | 1s | 3s | GLIBC guard passed inside build step |
| device | 1m54s | 6s | 4s | n/a | 1m31s | 2s | 1s | 4s | GLIBC guard passed inside build step |
| device-video-full | 2m03s | 3s | 4s | 2s | 1m43s | 2s | 1s | 3s | GLIBC guard passed inside build step |

Interpretation:

- Provisional keep pending a note-only repeat.
- Folding the check preserves the GLIBC guard and main artifacts.
- This is at the fast end of the accepted-state band and better than the
  passive accepted-state sample `2e69496` (`2m23s`, tail job `2m17s`), but a
  single fast run can be noise.

Repeat result:

- Note-only commit: `07edc56`
- Run: `27486596370`
- Conclusion: success.
- Workflow wall time: `2m25s` by run timestamps (`03:01:57Z` to `03:04:22Z`).
- Log check: all four ARM jobs printed `==> max GLIBC symbol version:
  GLIBC_2.31` inside `Build ARM binary` before `Upload ARM binary`.

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | FFmpeg cache | Build ARM binary | Upload ARM binary | Upload size history | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|--------------|------------------|-------------------|---------------------|-------------------------|-------|
| host-dev | 43s | 9s | 9s | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache 7s |
| fast | 1m45s | 4s | 6s | n/a | 1m17s | 2s | 2s | 6s | GLIBC guard passed inside build step |
| fast-video | 1m36s | 3s | 4s | 2s | 1m18s | 1s | 1s | 3s | GLIBC guard passed inside build step |
| device | 1m51s | 2s | 5s | n/a | 1m27s | 2s | 1s | 4s | GLIBC guard passed inside build step |
| device-video-full | 2m20s | 3s | 6s | 3s | 1m49s | 3s | 1s | 6s | GLIBC guard passed inside build step |

Interpretation:

- Reject. Correctness is fine, but timing did not hold: the repeat was slower
  than recent accepted-state samples and the tail job regressed to `2m20s`.
- The separate `Check ARM shared libraries` step costs only about `0s`-`1s` and
  gives clearer logs for the GLIBC guard, so restore it.

Restoration run after revert:

- Commit: `7a60947`
- Run: `27486676209`
- Conclusion: success.
- Workflow wall time: `2m13s` by run timestamps (`03:05:37Z` to `03:07:50Z`).

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | FFmpeg cache | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|--------------|------------------|----------------------------|-------------------|---------------------|-------------------------|-------|
| host-dev | 43s | 11s | 8s | n/a | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache 6s |
| fast | 1m35s | 3s | 4s | n/a | 1m15s | 1s | 1s | 1s | 4s | GLIBC guard passed |
| fast-video | 1m47s | 2s | 4s | 1s | 1m23s | 1s | 1s | 0s | 9s | GLIBC guard passed |
| device | 1m25s | 2s | 3s | n/a | 1m04s | 1s | 2s | 1s | 4s | GLIBC guard passed |
| device-video-full | 2m09s | 2s | 3s | 1s | 1m46s | 1s | 1s | 6s | 3s | GLIBC guard passed |

Interpretation:

- Restoration is green and faster than the folded repeat (`2m13s` versus
  `2m25s`, tail `2m09s` versus `2m20s`).
- Keep the separate shared-library check.

Passive accepted-state sample:

- Note-only commit: `32b1375`
- Run: `27486740427`
- Conclusion: success.
- Workflow wall time: `2m15s` by run timestamps (`03:08:55Z` to `03:11:10Z`).

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | FFmpeg cache | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|--------------|------------------|----------------------------|-------------------|---------------------|-------------------------|-------|
| host-dev | 24s | 2s | 5s | n/a | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache 5s |
| fast | 1m31s | 3s | 3s | n/a | 1m11s | 2s | 1s | 1s | 3s | GLIBC guard passed |
| fast-video | 1m51s | 5s | 4s | 2s | 1m26s | 1s | 1s | 1s | 4s | GLIBC guard passed |
| device | 1m53s | 3s | 4s | n/a | 1m36s | 1s | 1s | 0s | 3s | GLIBC guard passed |
| device-video-full | 2m10s | 3s | 5s | 2s | 1m43s | 1s | 1s | 2s | 5s | GLIBC guard passed |

Interpretation:

- Accepted state remains green and around `2m10s`-`2m15s` wall time when cache
  restore behaves.
- `device-video-full` remains the tail, dominated by the `release-device`
  build step.

## Experiment 28: make release-device use thin LTO

Change:

- Change `release-device` from fat LTO plus `codegen-units = 1` to the inherited
  `release` profile settings: thin LTO and default parallel codegen.
- Keep the same CI matrix, host checks, ARM builds, GLIBC guard, and artifact
  uploads.

Hypothesis:

- The two device jobs are the long pole because `release-device` uses fat LTO
  and single-CGU codegen. Thin LTO should reduce `device` and
  `device-video-full` compile time.

Risk:

- This changes the production device artifact profile, not just CI mechanics.
- Binary size may increase and runtime performance may differ from the previous
  A3 size winner. Treat this as a trial unless timing and size output justify
  the trade.

Measure:

- All jobs must pass.
- GLIBC max must remain `GLIBC_2.31`.
- Compare device job build durations and binary-size output against accepted
  state.

First result:

- Commit: `93b6728`
- Run: `27486809026`
- Conclusion: success.
- Workflow wall time: `3m24s` by run timestamps (`03:12:20Z` to `03:15:44Z`).
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Cache note: ARM target caches restored exact primary keys and were not saved
  after the run, even though the profile changed in `Cargo.toml`. The target
  cache key does not include `magik-gui/Cargo.toml`.

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | FFmpeg cache | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|--------------|------------------|----------------------------|-------------------|---------------------|-------------------------|-------|
| host-dev | 21s | 2s | 4s | n/a | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache 2s |
| fast | 1m32s | 4s | 3s | n/a | 1m15s | 0s | 1s | 1s | 3s | unchanged release profile; binary `6.40 MiB` |
| fast-video | 1m36s | 3s | 4s | 1s | 1m17s | 1s | 1s | 1s | 4s | unchanged release profile; binary `9.99 MiB` |
| device | 3m07s | 2s | 4s | n/a | 2m51s | 0s | 1s | 1s | 3s | binary grew to `6.40 MiB`; target cache exact hit was not saved |
| device-video-full | 3m20s | 3s | 3s | 2s | 2m59s | 0s | 2s | 0s | 4s | binary grew to `9.99 MiB`; target cache exact hit was not saved |

Interpretation:

- Reject immediately. The device jobs became much slower than the accepted
  state (`3m07s`/`3m20s` versus roughly `1m53s`/`2m10s` in the prior sample),
  and the device binaries lost the smaller fat-LTO size advantage.
- Restore `release-device` to fat LTO plus single CGU.
- Follow-up lead: include `magik-gui/Cargo.toml` in the target cache key before
  any future profile-shape experiments, because profile changes currently do
  not invalidate the exact target cache key.

Restoration run after revert:

- Commit: `6e02ef6`
- Run: `27486908969`
- Conclusion: success.
- Workflow wall time: `2m12s` by run timestamps (`03:17:18Z` to `03:19:30Z`).

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | FFmpeg cache | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Cache ARM build outputs | Notes |
|-----|----------|------------------------|----------------------|--------------|------------------|----------------------------|-------------------|---------------------|-------------------------|-------|
| host-dev | 48s | 11s | 11s | n/a | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache 7s |
| fast | 1m46s | 3s | 6s | n/a | 1m19s | 1s | 2s | 1s | 5s | GLIBC guard passed |
| fast-video | 1m47s | 3s | 3s | 1s | 1m23s | 1s | 1s | 1s | 4s | GLIBC guard passed |
| device | 1m55s | 3s | 3s | n/a | 1m39s | 1s | 1s | 0s | 3s | GLIBC guard passed |
| device-video-full | 2m07s | 3s | 9s | 2s | 1m41s | 1s | 1s | 1s | 4s | GLIBC guard passed |

Interpretation:

- Restoration is green and back in the accepted band.
- `release-device` fat LTO plus single CGU remains the accepted profile.

## Experiment 29: save per-SHA ARM target caches

Change:

- Replace the monolithic ARM `actions/cache` target step with explicit
  `actions/cache/restore` and `actions/cache/save`.
- Use a per-commit primary key ending in `${{ github.sha }}` and stable
  restore prefixes so each successful run can publish a fresh target cache for
  the next PR commit.
- Include `magik-gui/Cargo.toml` in the target-cache hash so profile changes
  invalidate the stable prefix.
- Keep the same matrix, host checks, ARM builds, GLIBC guard, and artifacts.

Hypothesis:

- The current exact target cache key is effectively read-only after the first
  save. Source-only commits restore an old cache, rebuild the local crate, then
  skip saving because the exact key already exists.
- Saving a per-SHA cache should improve subsequent PR commits, especially
  note-only or workflow-only repeats, by restoring fresher local crate outputs.

Risk:

- Saving 200-260 MiB target caches on every ARM job may add enough upload time
  to offset shorter builds.
- Cache storage churn may increase. This is a timing experiment, not an obvious
  keep.

Measure:

- First run may be noisy because the key shape changes.
- Repeat with a note-only commit and compare build-step reduction against new
  save-step overhead.

First result:

- Commit: `bebda00`
- Run: `27486984972`
- Conclusion: success.
- Workflow wall time: `2m07s` by run timestamps (`03:21:15Z` to `03:23:22Z`).
- Cache behavior: all ARM jobs restored an older prefix cache, then saved a
  fresh per-SHA target cache.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.

Job timings:

| Job | Duration | Restore ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Save ARM build outputs | Notes |
|-----|----------|---------------------------|------------------|----------------------------|-------------------|---------------------|------------------------|-------|
| host-dev | 29s | n/a | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m34s | 4s | 1m05s | 3s | 2s | 1s | 4s | restored old prefix; saved `237.6 MB`; GLIBC guard passed |
| fast-video | 1m49s | 6s | 1m14s | 1s | 2s | 2s | 5s | restored old prefix; saved `267.9 MB`; GLIBC guard passed |
| device | 1m59s | 2s | 1m34s | 1s | 1s | 1s | 8s | restored old prefix; saved `236.3 MB`; GLIBC guard passed |
| device-video-full | 2m02s | 3s | 1m41s | 0s | 1s | 1s | 4s | restored old prefix; saved `266.6 MB`; GLIBC guard passed |

Interpretation:

- Promising but not accepted yet. The first run beat the recent accepted-state
  tail even while paying save overhead, but the important test is the next
  note-only run restoring the freshly saved per-SHA cache.

Repeat result:

- Note-only commit: `fb6d6b4`
- Run: `27487046000`
- Conclusion: success.
- Workflow wall time: `2m11s` by run timestamps (`03:24:21Z` to `03:26:32Z`).
- Cache behavior: ARM jobs restored the fresh `bebda00` per-SHA caches via the
  stable restore prefix, then saved new `fb6d6b4` caches.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.

Job timings:

| Job | Duration | Restore ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Save ARM build outputs | Notes |
|-----|----------|---------------------------|------------------|----------------------------|-------------------|---------------------|------------------------|-------|
| host-dev | 23s | n/a | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m37s | 2s | 1m14s | 1s | 1s | 1s | 3s | restored fresh per-SHA prefix, but still recompiled local crate |
| fast-video | 1m39s | 4s | 1m18s | 1s | 1s | 0s | 4s | restored fresh per-SHA prefix, but still recompiled local crate |
| device | 2m06s | 3s | 1m40s | 0s | 6s | 1s | 3s | restored fresh per-SHA prefix, but still recompiled local crate |
| device-video-full | 2m08s | 4s | 1m42s | 1s | 1s | 1s | 4s | restored fresh per-SHA prefix, but still recompiled local crate |

Interpretation:

- Reject. The repeat landed in the same accepted-state band (`2m11s` wall,
  `2m08s` tail) while adding cache-save work and cache churn.
- Fresh target caches did not prevent Cargo from recompiling the local crate on
  a note-only commit, likely because checkout/source timestamps still make the
  package newer than restored outputs.
- Restore the simpler single `actions/cache` target step.

Restoration run after revert:

- Commit: `ae94e5b`
- Run: `27487120245`
- Conclusion: success.
- Workflow wall time: `2m12s` by run timestamps (`03:28:09Z` to `03:30:21Z`).

Job timings:

| Job | Duration | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 35s | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m24s | 3s | 1m06s | 1s | 1s | 1s | GLIBC guard passed |
| fast-video | 1m44s | 4s | 1m23s | 1s | 1s | 1s | GLIBC guard passed |
| device | 1m54s | 3s | 1m36s | 1s | 1s | 1s | GLIBC guard passed |
| device-video-full | 2m09s | 4s | 1m46s | 0s | 1s | 1s | GLIBC guard passed |

Interpretation:

- Restoration is green and back to accepted-state behavior.
- Keep the simpler single cache step.

## Experiment 30: fat LTO with parallel codegen for release-device

Change:

- Keep `release-device` on fat LTO, but change `codegen-units` from `1` to
  `16`.
- Include `magik-gui/Cargo.toml` in the ARM target cache key so this profile
  shape gets its own primary cache.
- Keep the same matrix, host checks, ARM builds, GLIBC guard, and artifacts.

Hypothesis:

- `codegen-units = 1` may be making the device jobs slower before the final fat
  LTO link. More codegen units could recover compile parallelism while keeping
  fat LTO's smaller binary behavior closer than the thin-LTO trial.

Risk:

- This changes production artifact optimization behavior. Binary size and
  runtime performance may move.
- The first run will use a new target-cache key because `Cargo.toml` joins the
  hash; compare the repeat more heavily than the seed run.

Measure:

- All jobs must pass.
- GLIBC max must remain `GLIBC_2.31`.
- Compare `device` and `device-video-full` build time and binary-size output
  against the accepted-state samples.

First result:

- Commit: `a6c5268`
- Run: `27487199513`
- Conclusion: success.
- Workflow wall time: `2m40s` by run timestamps (`03:32:10Z` to `03:34:50Z`).
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Cache behavior: ARM target caches restored from the previous broad restore
  key and saved new primary caches because `Cargo.toml` joined the hash.

Job timings:

| Job | Duration | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 27s | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m43s | 5s | 1m15s | 1s | 2s | 1s | unchanged release profile; saved new target cache |
| fast-video | 1m53s | 8s | 1m12s | 1s | 1s | 1s | unchanged release profile; cache restore was slow; saved new target cache |
| device | 2m17s | 4s | 1m52s | 1s | 1s | 1s | binary grew to `5.75 MiB`; saved new target cache |
| device-video-full | 2m36s | 6s | 2m00s | 1s | 2s | 1s | binary grew to `9.30 MiB`; saved new target cache |

Interpretation:

- Reject without a repeat. The device build steps themselves regressed versus
  accepted-state samples (`1m52s`/`2m00s` here versus about `1m36s`/`1m46s`),
  and the device binaries grew.
- Restore `release-device` to `codegen-units = 1`.
- Restore the previous ARM target cache key; adding `Cargo.toml` is useful for
  profile-shape correctness but creates a cache churn/cold-key cost and did not
  reveal a speed win here.

Restoration run after revert:

- Commit: `e3db882`
- Run: `27487277164`
- Conclusion: success.
- Workflow wall time: `2m19s` by run timestamps (`03:36:17Z` to
  `03:38:36Z`).
- Cache behavior: all ARM target caches hit their primary keys again; no target
  caches were saved.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Device binary sizes returned to accepted-state values: `5.60 MiB` for
  `device`, `9.09 MiB` for `device-video-full`.

Job timings:

| Job | Duration | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 30s | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m43s | 7s | 1m16s | 1s | 6s | 0s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m46s | 5s | 1m18s | 0s | 2s | 2s | primary target-cache hit; binary `9.99 MiB` |
| device | 1m48s | 8s | 1m27s | 1s | 1s | 1s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 2m15s | 4s | 1m57s | 0s | 2s | 0s | primary target-cache hit; binary `9.09 MiB` |

Interpretation:

- Restoration is green and back to the accepted profile/cache shape.
- The `device-video-full` tail was `2m15s`, slower than the best accepted-state
  samples but still in the observed noisy band. Treat this as baseline noise,
  not a new regression.

Passive baseline sample:

- Note-only commit: `47ffc2e`
- Run: `27487369819`
- Conclusion: success.
- Workflow wall time: `2m06s` by run timestamps (`03:41:13Z` to
  `03:43:19Z`).
- Cache behavior: all ARM target caches hit their primary keys; no target caches
  were saved.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.

Job timings:

| Job | Duration | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 34s | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m29s | 3s | 1m12s | 0s | 1s | 1s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m36s | 3s | 1m13s | 1s | 1s | 0s | primary target-cache hit; binary `9.99 MiB` |
| device | 1m51s | 3s | 1m32s | 1s | 2s | 0s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 2m02s | 3s | 1m42s | 0s | 2s | 1s | primary target-cache hit; binary `9.09 MiB` |

Interpretation:

- This is a strong accepted-state baseline sample and shows the target cache can
  hit without saving, but Cargo still spends `1m42s` in the tail build step on a
  note-only commit.

## Experiment 31: narrow build-script rerun inputs

Change:

- Add explicit `cargo:rerun-if-changed=build.rs`.
- Add explicit `cargo:rerun-if-env-changed=CARGO_FEATURE_BENCH_SCENES` beside
  the existing `MISTER_UI_BUILD_SCOPE` env dependency.
- Keep the same matrix, caches, builds, GLIBC guard, and artifacts.

Hypothesis:

- `magik-gui/build.rs` only depends on its own source and two environment
  values. If Cargo was conservatively treating the whole package as build-script
  input, narrowing the rerun set may let restored target outputs survive
  note-only commits better.

Risk:

- Low. Source-file changes should still rebuild normal Rust targets, and feature
  or UI-scope changes are explicit env inputs. This does not skip any build or
  test.

Measure:

- First run validates correctness after the `build.rs` edit.
- A follow-up note-only run is the important measurement: if the build-script
  input narrowing works, ARM `Build ARM binary` steps should drop sharply without
  changing artifacts or GLIBC compatibility.

First result:

- Commit: `3087e33`
- Run: `27487434826`
- Conclusion: success.
- Workflow wall time: `2m33s` by run timestamps (`03:44:39Z` to
  `03:47:12Z`).
- Cache behavior: changing `build.rs` changed the ARM target-cache key. Jobs
  restored from the previous prefix and saved new primary caches.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.

Job timings:

| Job | Duration | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Post target cache | Notes |
|-----|----------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------------------|-------|
| host-dev | 37s | n/a | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m49s | 6s | 1m17s | 1s | 2s | 1s | 6s | restored old prefix; saved new target cache |
| fast-video | 1m46s | 5s | 1m13s | 2s | 2s | 1s | 5s | restored old prefix; saved new target cache |
| device | 2m05s | 5s | 1m36s | 1s | 2s | 2s | 4s | restored old prefix; saved new target cache |
| device-video-full | 2m23s | 4s | 1m54s | 1s | 1s | 0s | 5s | restored old prefix; saved new target cache |

Interpretation:

- Green, but not a performance result yet. The first run paid the expected
  target-cache-key churn from changing `build.rs`.
- Run a note-only repeat to see whether the newly narrowed build-script inputs
  let restored primary target caches avoid the usual local-crate rebuild.

Repeat result:

- Note-only commit: `405a4c4`
- Run: `27487512046`
- Conclusion: success.
- Workflow wall time: `2m16s` by run timestamps (`03:48:40Z` to
  `03:50:56Z`).
- Cache behavior: all ARM target caches hit exact primary keys; no target caches
  were saved.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.

Job timings:

| Job | Duration | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 52s | n/a | n/a | n/a | n/a | n/a | all host checks green; slow host cache/setup sample |
| fast | 1m47s | 6s | 1m15s | 1s | 2s | 6s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m54s | 5s | 1m24s | 1s | 2s | 1s | primary target-cache hit; binary `9.99 MiB` |
| device | 2m04s | 3s | 1m45s | 1s | 1s | 1s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 2m11s | 9s | 1m42s | 1s | 1s | 1s | primary target-cache hit; binary `9.09 MiB` |

Interpretation:

- Reject. Narrowing build-script rerun inputs did not make a note-only commit
  cheap; exact primary target-cache hits still spent normal time in
  `Build ARM binary`.
- Restore `build.rs` to its previous minimal env dependency declarations.

Restoration run after revert:

- Commit: `7dd1953`
- Run: `27487579088`
- Conclusion: success.
- Workflow wall time: `2m09s` by run timestamps (`03:52:07Z` to
  `03:54:16Z`).
- Cache behavior: ARM target caches hit exact primary keys again; no target
  caches were saved.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.

Job timings:

| Job | Duration | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 33s | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m23s | 3s | 1m06s | 1s | 0s | 1s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m49s | 4s | 1m14s | 1s | 1s | 1s | primary target-cache hit; binary `9.99 MiB` |
| device | 1m59s | 4s | 1m36s | 1s | 2s | 1s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 1m55s | 4s | 1m36s | 1s | 1s | 0s | primary target-cache hit; binary `9.09 MiB` |

Interpretation:

- Restoration is green and back to accepted behavior.
- The tail was `1m59s`, one of the best accepted-state samples tonight.

## Experiment 32: remove host gcc/g++ from cross Docker image

Change:

- Remove direct `gcc` and `g++` packages from
  `magik-gui/Dockerfile.cross-armv7`.
- Keep `gcc-arm-linux-gnueabihf`, `g++-arm-linux-gnueabihf`,
  `libc6-dev-armhf-cross`, `libclang-dev`, `make`, `pkg-config`, and
  `ca-certificates`.
- Keep the same matrix, target caches, builds, GLIBC guard, and artifacts.

Hypothesis:

- The image build step repeatedly installs host and ARM compiler packages in
  every GitHub-hosted ARM job. Target compilation should need the ARM cross
  compilers, not direct native `gcc`/`g++` packages. Removing host compilers may
  reduce Docker package install work and image size without weakening the build.

Risk:

- Some build script or configure check may need native host C/C++ compilers
  inside the container. If so, the failure is honest and this gets reverted.
- The first run will churn the Dockerfile-based ARM target-cache key, so a warm
  repeat is required before judging speed.

Measure:

- All jobs must pass.
- GLIBC max must remain `GLIBC_2.31`.
- Compare Docker package install/build step time and full workflow wall time
  against the restoration sample above and accepted Docker-trim samples from
  Experiment 7.

Result:

- Commit: `d6ae7da`
- Run: `27487640733`
- Conclusion: failure.
- Host job passed in `34s`.
- All ARM jobs failed in `Build ARM binary`:
  - `fast`: `48s`
  - `fast-video`: `57s`
  - `device`: `49s`
  - `device-video-full`: `1m03s`
- Representative failure: `error: linker 'cc' not found` with `No such file or
  directory (os error 2)`.

Interpretation:

- Reject immediately. The target build still needs a native `cc` inside the
  cross container for host-side build artifacts, even though the final target
  linker is `arm-linux-gnueabihf-gcc`.
- Restore direct host `gcc`/`g++` packages.

Restoration run after revert:

- Commit: `8adfeea`
- Run: `27487683553`
- Conclusion: success.
- Workflow wall time: `2m14s` by run timestamps (`03:57:37Z` to
  `03:59:51Z`).
- Cache behavior: ARM target caches hit exact primary keys again; no target
  caches were saved.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.

Job timings:

| Job | Duration | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 41s | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m36s | 3s | 1m13s | 1s | 1s | 0s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m50s | 5s | 1m21s | 1s | 2s | 2s | primary target-cache hit; binary `9.99 MiB` |
| device | 1m59s | 5s | 1m35s | 1s | 1s | 1s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 2m10s | 9s | 1m33s | 0s | 2s | 1s | primary target-cache hit; binary `9.09 MiB` |

Interpretation:

- Restoration is green and back to accepted behavior.
- The failed image trim proved native `cc` is required, so the next narrower
  Docker trim should keep host `gcc` and remove only host `g++`.

## Experiment 33: remove only host g++ from cross Docker image

Change:

- Remove direct native `g++` from `magik-gui/Dockerfile.cross-armv7`.
- Keep native `gcc` so `/usr/bin/cc` exists.
- Keep ARM cross `gcc`/`g++`, ARM sysroot, `libclang-dev`, `make`,
  `pkg-config`, and certs.
- Keep the same matrix, target caches, builds, GLIBC guard, and artifacts.

Hypothesis:

- Experiment 32 showed native `cc` is required. It did not prove native C++ is
  required. If no host-side build step needs `c++`/`g++`, dropping only native
  `g++` may still reduce the helper image package set safely.

Risk:

- A host-side C++ link step may need native `g++`. If so, reject and restore.
- The first run again changes the Dockerfile-based cache key, so a warm repeat
  is required if the seed is green.

Measure:

- All jobs must pass.
- GLIBC max must remain `GLIBC_2.31`.
- Compare Docker package install time and warm-repeat wall time against the
  accepted-state band.

First result:

- Commit: `7833a2b`
- Run: `27487758455`
- Conclusion: success.
- Workflow wall time: `3m14s` by run timestamps (`04:01:12Z` to
  `04:04:26Z`).
- Cache behavior: Dockerfile hash changed. ARM target caches restored from the
  previous prefix and saved new primary caches. Video FFmpeg cache missed under
  the new Dockerfile hash.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Docker package install layer still ran in every ARM job:
  - `fast`: `24.2s`
  - `device`: `23.4s`
  - `fast-video`: `29.2s`
  - `device-video-full`: `23.9s`

Job timings:

| Job | Duration | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Post target cache | Notes |
|-----|----------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------------------|-------|
| host-dev | 35s | n/a | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m32s | 3s | 1m13s | 1s | 1s | 0s | 3s | restored old prefix; saved new target cache; binary `6.40 MiB` |
| fast-video | 2m26s | 5s | 1m57s | 0s | 2s | 1s | 4s | FFmpeg cache miss; saved new target cache and FFmpeg cache; binary `9.99 MiB` |
| device | 1m53s | 4s | 1m30s | 1s | 1s | 1s | 5s | restored old prefix; saved new target cache; binary `5.60 MiB` |
| device-video-full | 3m09s | 7s | 2m33s | 1s | 2s | 1s | 6s | FFmpeg cache miss; saved new target cache; binary `9.09 MiB` |

Interpretation:

- Seed is green, which proves native `g++` is not required for these builds.
- Not a speed verdict because Dockerfile hash churn forced new target and FFmpeg
  caches. Run a note-only warm repeat using the new caches.

Manual repeat 1:

- Commit: `dad394a`
- Run: `27487869606`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `4m37s` by run timestamps (`04:06:32Z` to
  `04:11:09Z`).
- Cache behavior: not a valid warm-repeat sample. Host, ARM target, and video
  FFmpeg caches missed under the manual-dispatch/cache-key shape and saved new
  caches.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Docker package install layer still ran in every ARM job:
  - `fast`: `23.0s`
  - `device`: `22.2s`
  - `fast-video`: `23.5s`
  - `device-video-full`: `22.1s`

Job timings:

| Job | Duration | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 57s | n/a | n/a | n/a | n/a | n/a | host target cache miss; saved cache |
| fast | 3m04s | miss | 2m48s | 1s | 1s | 1s | saved target cache; binary `6.40 MiB` |
| fast-video | 4m25s | miss | 3m59s | 1s | 2s | 1s | FFmpeg miss; saved target and FFmpeg caches; binary `9.99 MiB` |
| device | 3m18s | miss | 3m00s | 1s | 1s | 1s | saved target cache; binary `5.60 MiB` |
| device-video-full | 4m32s | miss | 4m13s | 1s | 2s | 1s | FFmpeg miss; saved target and FFmpeg caches; binary `9.09 MiB` |

Interpretation:

- Inconclusive for speed. The run is green, but it rebuilt and saved caches, so
  it is not comparable to accepted warm-cache samples.
- Run a second manual dispatch on the same head to use the newly saved caches.

Manual repeat 2:

- Commit: `dad394a`
- Run: `27487977278`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `3m13s` by run timestamps (`04:12:12Z` to
  `04:15:25Z`).
- Cache behavior: host, ARM target, and FFmpeg caches hit primary keys; no
  target or FFmpeg caches were saved.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Docker package install layer still ran in every ARM job:
  - `fast`: `23.1s`
  - `device`: `23.8s`
  - `fast-video`: `23.2s`
  - `device-video-full`: `24.4s`

Job timings:

| Job | Duration | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 30s | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache hit |
| fast | 1m51s | 3s | 1m27s | 1s | 1s | 0s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m34s | 8s | 1m10s | 1s | 1s | 1s | primary target-cache and FFmpeg hits; binary `9.99 MiB` |
| device | 3m07s | 5s | 2m29s | 1s | 2s | 1s | primary target-cache hit; binary `5.60 MiB`; outlier |
| device-video-full | 1m59s | 4s | 1m38s | 1s | 1s | 1s | primary target-cache and FFmpeg hits; binary `9.09 MiB` |

Interpretation:

- Still inconclusive. The cache state is finally fair, and the video jobs look
  excellent, but the plain `device` job is a large outlier: it took `3m07s`
  while the heavier `device-video-full` job finished in `1m59s`.
- Run one more warm repeat before accepting or rejecting the host-`g++` trim.

Manual repeat 3:

- Commit: `dad394a`
- Run: `27488084343`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m27s` by run timestamps (`04:17:52Z` to
  `04:20:19Z`).
- Cache behavior: host, ARM target, and FFmpeg caches hit primary keys; no
  target or FFmpeg caches were saved.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.
- Docker package install layer still ran in every ARM job:
  - `fast`: `22.7s`
  - `device`: `23.3s`
  - `fast-video`: `23.6s`
  - `device-video-full`: `25.1s`

Job timings:

| Job | Duration | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 37s | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache hit |
| fast | 1m49s | 5s | 1m20s | 1s | 2s | 1s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m42s | 5s | 1m14s | 1s | 2s | 1s | primary target-cache and FFmpeg hits; binary `9.99 MiB` |
| device | 1m52s | 3s | 1m35s | 1s | 1s | 0s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 2m21s | 6s | 1m45s | 1s | 2s | 1s | primary target-cache and FFmpeg hits; binary `9.09 MiB` |

Interpretation:

- Accept. Removing native host `g++` is build-safe across all profiles and keeps
  the warm-cache tail within the accepted band.
- This is a small image/package-set trim, not a dramatic timing win: the apt
  layer still costs roughly `23-25s` because the ARM cross C++ package remains
  required. The main value is removing one unneeded host compiler package
  without weakening the CI matrix.
- The previous `device` `3m07s` repeat was runner noise; the next identical
  repeat brought `device` back to `1m52s`.

## Experiment 34: link ARM builds with lld

Change:

- Add `lld` to `magik-gui/Dockerfile.cross-armv7`.
- Pass `-C link-arg=-fuse-ld=lld` through `magik-gui/build-arm.sh` while keeping
  the same ARM cross GCC linker driver.
- Keep the same host checks, ARM matrix, features, GLIBC guard, and artifacts.

Hypothesis:

- Bun uses `lld` explicitly for its Rust build integration. Our warm-cache ARM
  jobs still spend most of their tail in `Build ARM binary`, and the
  `release-device` profile uses fat LTO with `codegen-units = 1`, so a faster
  linker may reduce the slowest device build without skipping any work.

Risk:

- The ARM GCC driver may not accept or find `lld` correctly in the cross
  container.
- `lld` may emit a binary with different dynamic symbol requirements; the
  existing `check-arm-shared-libs.sh` must still cap the binary at
  `GLIBC_2.31`.
- Adding the package increases the Docker apt layer, so this needs a warm repeat
  after Dockerfile/hash churn before judging timing.

Measure:

- All jobs must pass.
- GLIBC max must remain `GLIBC_2.31`.
- Compare warm-repeat `Build ARM binary` and total wall time against accepted
  warm samples, especially `device` and `device-video-full`.

Result:

- Commit: `34aa082`
- Run: `27488182187`
- Event: `workflow_dispatch`
- Conclusion: failure.
- Host job passed in `37s`.
- All ARM jobs failed in `Build ARM binary`:
  - `fast`: `2m18s`
  - `device`: `2m36s`
  - `fast-video`: `3m11s`
  - `device-video-full`: `3m32s`
- Docker package install layer with `lld` took `24.6s` in the `fast` job and
  installed `lld` / `lld-10`.
- Representative failure:
  `linking with arm-linux-gnueabihf-gcc failed`, with the linker invocation
  ending in `-fuse-ld=lld`, then `collect2: fatal error: cannot find 'ld'`.

Interpretation:

- Reject. Bun's lld setup is not directly portable to this CI path because Bun
  drives Rust linking through a clang/lld configuration, while this repo's ARM
  build uses the Ubuntu 20.04 `arm-linux-gnueabihf-gcc` cross driver.
- A future lld attempt would need a more explicit cross-linker configuration,
  not just `-fuse-ld=lld`; that is too risky to keep after this matrix failure.
- Restore the previous GCC/BFD linker path and remove the `lld` package.

Restoration run after revert:

- Commit: `a006fa9`
- Run: `27488275985`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m10s` by run timestamps (`04:27:59Z` to
  `04:30:09Z`).
- Cache behavior: host, ARM target, and FFmpeg caches hit primary keys; no
  target or FFmpeg caches were saved.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.
- Docker package install layer returned to the accepted package set:
  - `fast`: `24.7s`
  - `device`: `24.9s`
  - `fast-video`: `23.8s`
  - `device-video-full`: `22.8s`

Job timings:

| Job | Duration | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 24s | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache hit |
| fast | 1m40s | 5s | 1m14s | 1s | 2s | 1s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m48s | 5s | 1m21s | 1s | 2s | 1s | primary target-cache and FFmpeg hits; binary `9.99 MiB` |
| device | 1m55s | 4s | 1m31s | 1s | 1s | 1s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 2m00s | 3s | 1m41s | 1s | 1s | 1s | primary target-cache and FFmpeg hits; binary `9.09 MiB` |

Interpretation:

- Restoration is green and back to accepted behavior.
- The lld experiment remains rejected; the PR head is no longer carrying the
  failed linker/package change.

## Experiment 35: remove explicit ARM rustup target install

Change:

- In the ARM matrix, change the `Install Rust toolchain` step from
  `rustup show && rustup target add armv7-unknown-linux-gnueabihf` to just
  `rustup show`.
- Keep the same `cross` build commands, ARM matrix, host checks, GLIBC guard,
  and artifacts.

Hypothesis:

- `cross` performs its own toolchain/container preparation before building, and
  recent build logs already show `cross` touching the stable toolchain and
  downloading `rust-src` inside `Build ARM binary`. The explicit host-side
  `rustup target add` may be redundant setup cost in every ARM job.

Risk:

- If `cross` relies on the host target std component already being installed,
  ARM jobs will fail with a missing target/std error. In that case, reject and
  restore the explicit target install.

Measure:

- All jobs must pass.
- GLIBC max must remain `GLIBC_2.31`.
- Compare `Install Rust toolchain` step duration and total warm-run wall time
  against accepted samples.

First result:

- Commit: `a269a47`
- Run: `27488353225`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m23s` by run timestamps (`04:31:54Z` to
  `04:34:17Z`).
- Cache behavior: host, ARM target, and FFmpeg caches hit primary keys; no
  target or FFmpeg caches were saved.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.
- `Build ARM binary` still downloaded `rust-src` in each ARM job, so removing
  `rustup target add` did not remove that `cross` preparation cost.
- Docker package install layer stayed in the normal range:
  - `fast`: `25.3s`
  - `device`: `23.8s`
  - `fast-video`: `24.0s`
  - `device-video-full`: `21.1s`

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 22s | 2s | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache hit |
| fast | 1m38s | 2s | 4s | 1m13s | 1s | 2s | 1s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 2m04s | 2s | 11s | 1m27s | 1s | 3s | 1s | primary target-cache and FFmpeg hits; binary `9.99 MiB` |
| device | 1m54s | 3s | 5s | 1m28s | 1s | 2s | 1s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 2m11s | 4s | 5s | 1m46s | 1s | 2s | 1s | primary target-cache and FFmpeg hits; binary `9.09 MiB` |

Interpretation:

- Green, but not yet a verdict. Omitting explicit `rustup target add` is safe in
  this sample, but total wall time did not clearly improve over the previous
  accepted/recovery band.
- Run one identical repeat before accepting or rejecting the setup trim.

Repeat result:

- Commit: `93ef4b1`
- Run: `27488421421`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m14s` by run timestamps (`04:35:23Z` to
  `04:37:37Z`).
- Cache behavior: host, ARM target, and FFmpeg caches hit primary keys; no
  target or FFmpeg caches were saved.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.
- `Build ARM binary` still downloaded `rust-src` in each ARM job.
- Docker package install layer stayed in the normal range:
  - `fast`: `23.2s`
  - `device`: `24.9s`
  - `fast-video`: `22.1s`
  - `device-video-full`: `25.3s`

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 37s | 2s | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache hit |
| fast | 1m24s | 2s | 4s | 1m05s | 1s | 1s | 1s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m41s | 3s | 3s | 1m21s | 1s | 1s | 1s | primary target-cache and FFmpeg hits; binary `9.99 MiB` |
| device | 1m32s | 2s | 4s | 1m12s | 0s | 2s | 1s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 2m08s | 3s | 4s | 1m38s | 0s | 2s | 1s | primary target-cache and FFmpeg hits; binary `9.09 MiB` |

Interpretation:

- Accept. The explicit `rustup target add armv7-unknown-linux-gnueabihf` is not
  required for these `cross` builds, and removing it keeps the full matrix green.
- This is a small setup trim, not a compile-time breakthrough. The `cross` build
  still downloads `rust-src`, and Docker/package plus Cargo build time remain
  the real tail.

## Experiment 36: declare rust-src in the Rust toolchain

Change:

- Add `rust-src` to `magik-gui/rust-toolchain.toml` components.
- Keep the explicit ARM `rustup target add` removed from the workflow.
- Keep the same `cross` build commands, ARM matrix, host checks, GLIBC guard,
  and artifacts.

Hypothesis:

- Experiment 35 removed the explicit target install safely, but each ARM
  `Build ARM binary` step still logged `info: downloading component rust-src`.
  Declaring `rust-src` in the pinned toolchain may install that component during
  the existing `rustup show` step and avoid per-build `cross` rust-src work.

Risk:

- The cost may simply move from `Build ARM binary` to `Install Rust toolchain`,
  giving no wall-time improvement.
- Host-dev may also pay for `rust-src` even though it does not need it.

Measure:

- All jobs must pass.
- GLIBC max must remain `GLIBC_2.31`.
- Compare `Install Rust toolchain`, `Build ARM binary`, and workflow wall time
  against Experiment 35.
- Check whether `Build ARM binary` still logs `downloading component rust-src`.

First result:

- Commit: `e483978`
- Run: `27488498552`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m19s` by run timestamps (`04:39:31Z` to
  `04:41:50Z`).
- Cache behavior: `rust-toolchain.toml` changed the target-cache key. ARM target
  caches restored from the previous prefix and saved new primary caches.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.
- `rust-src` moved to `Install Rust toolchain`; `Build ARM binary` no longer
  logged `downloading component rust-src`.

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Post target cache | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------------------|-------|
| host-dev | 55s | 13s | n/a | n/a | n/a | n/a | n/a | saved new host target cache | host paid for rust-src |
| fast | 1m30s | 3s | 4s | 1m08s | 0s | 2s | 0s | saved new target cache | binary `6.40 MiB` |
| fast-video | 1m46s | 2s | 4s | 1m13s | 1s | 1s | 1s | saved new target cache | binary `9.99 MiB` |
| device | 2m13s | 5s | 5s | 1m38s | 1s | 2s | 1s | saved new target cache | binary `5.60 MiB`; tail |
| device-video-full | 2m04s | 3s | 5s | 1m32s | 2s | 2s | 1s | saved new target cache | binary `9.09 MiB` |

Interpretation:

- Green, but not a performance verdict because the toolchain hash churned target
  caches and caused target-cache saves.
- Run one warm repeat on the same head.

Repeat result:

- Commit: `e483978`
- Run: `27488556768`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m22s` by run timestamps (`04:42:27Z` to
  `04:44:49Z`).
- Cache behavior: host, ARM target, and FFmpeg caches hit primary keys; no
  target or FFmpeg caches were saved.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.
- `rust-src` was still downloaded in `Install Rust toolchain` for every job,
  including host-dev. `Build ARM binary` no longer downloaded it.

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 24s | 3s | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache hit |
| fast | 1m33s | 5s | 3s | 1m10s | 0s | 2s | 0s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m21s | 2s | 3s | 1m03s | 0s | 1s | 1s | primary target-cache and FFmpeg hits; binary `9.99 MiB` |
| device | 1m48s | 3s | 3s | 1m32s | 1s | 1s | 0s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 2m16s | 5s | 6s | 1m44s | 1s | 2s | 1s | primary target-cache and FFmpeg hits; binary `9.09 MiB` |

Interpretation:

- Reject. Declaring `rust-src` merely moves the repeated download from the
  `cross` build phase to the workflow toolchain phase. It still happens every
  job, including host-dev, and the warm-repeat wall time did not improve over
  Experiment 35.
- Restore `rust-toolchain.toml` to `rustfmt` + `clippy` only.

Restoration run after revert:

- Commit: `7202339`
- Run: `27488627899`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m01s` by run timestamps (`04:46:08Z` to
  `04:48:09Z`).
- Cache behavior: host, ARM target, and FFmpeg caches hit primary keys; no
  target or FFmpeg caches were saved.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.
- `rust-src` returned to being downloaded inside `Build ARM binary`.

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 33s | 2s | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache hit |
| fast | 1m33s | 2s | 5s | 1m08s | 1s | 2s | 1s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m42s | 2s | 5s | 1m13s | 1s | 2s | 1s | primary target-cache and FFmpeg hits; binary `9.99 MiB` |
| device | 1m42s | 3s | 4s | 1m18s | 1s | 2s | 1s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 1m55s | 2s | 5s | 1m28s | 1s | 2s | 1s | primary target-cache and FFmpeg hits; binary `9.09 MiB` |

Interpretation:

- Restoration is green and faster than the rust-src warm repeat.
- The rust-src component experiment remains rejected.

## Experiment 37: cache rust-src for ARM cross builds

Change:

- Add an ARM-matrix cache step for
  `~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/src/rust`.
- Key it by runner OS and `magik-gui/rust-toolchain.toml`.
- Keep `rust-src` out of `rust-toolchain.toml`, keep the explicit ARM
  `rustup target add` removed, and keep the same build matrix and artifacts.

Hypothesis:

- Experiment 36 showed declaring `rust-src` as a toolchain component is not
  useful because every job pays in `Install Rust toolchain`. A narrower cache
  may let `cross` find the rust source tree without downloading it during
  `Build ARM binary`, while avoiding a host-dev penalty.

Risk:

- The cache path may not be sufficient for rustup/cross to consider `rust-src`
  installed.
- Cache restore/save overhead may be larger than the `rust-src` download.

Measure:

- All jobs must pass.
- GLIBC max must remain `GLIBC_2.31`.
- Compare rust-src cache restore/save time, `Build ARM binary`, and total wall
  time against Experiment 35 and the Experiment 36 recovery.
- Check whether `Build ARM binary` still logs `downloading component rust-src`.

Invalid seed:

- Commit: `d63fdfe`
- Run: `27488696988`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m24s` by run timestamps (`04:49:50Z` to
  `04:52:14Z`).
- Invalid reason: the new `Cache rust-src` step was inserted under `host-dev`,
  not under the ARM matrix. The ARM jobs therefore did not test the intended
  rust-src cache.
- Host-dev passed in `40s` and ran `Cache rust-src`; ARM jobs still used the
  accepted workflow shape and finished green (`fast` `1m27s`, `fast-video`
  `1m36s`, `device` `1m59s`, `device-video-full` `2m18s`).

Correction:

- Move the `Cache rust-src` step from `host-dev` to `arm-build`, immediately
  after the ARM `Install Rust toolchain` step.

Corrected seed result:

- Commit: `eff00fe`
- Run: `27488767712`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m28s` by run timestamps (`04:53:28Z` to
  `04:55:56Z`).
- Cache behavior: every ARM job missed the new `rust-src` cache. The `fast` job
  saved it successfully with key
  `rust-src-Linux-fc29b3ef445d17b09da9eec96ad0a054c0686b8070ba161499885a6a303742cc`;
  the other ARM jobs could not reserve the same key because that cache was
  already being created.
- `Build ARM binary` still logged `info: downloading component rust-src` in all
  ARM jobs on the seed run.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.

Job timings:

| Job | Duration | Install Rust toolchain | Cache rust-src | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|----------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 37s | n/a | n/a | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m32s | 2s | 0s miss | 4s | 1m06s | 1s | 2s | 1s | saved rust-src cache; binary `6.40 MiB` |
| fast-video | 1m42s | 3s | 0s miss | 5s | 1m13s | 1s | 2s | 1s | cache save skipped by key contention; binary `9.99 MiB` |
| device | 1m58s | 5s | 0s miss | 3s | 1m37s | 1s | 1s | 0s | cache save skipped by key contention; binary `5.60 MiB` |
| device-video-full | 2m17s | 3s | 0s miss | 4s | 1m55s | 0s | 1s | 1s | cache save skipped by key contention; binary `9.09 MiB` |

Warm repeat result:

- Commit: `eff00fe`
- Run: `27488855549`
- Event: `workflow_dispatch`
- Conclusion: failure.
- Workflow wall time: `42s` by run timestamps (`04:57:53Z` to `04:58:35Z`).
- The restored `rust-src` cache caused all ARM jobs to fail in `Build ARM
  binary`. Representative error from `device`:
  `error: failed to install component: 'rust-src', detected conflict:
  'lib/rustlib/src/rust/library/Cargo.lock'`.
- Diagnosis: caching only the source tree payload puts files in place, but does
  not update rustup's component metadata. `cross`/rustup still tries to install
  `rust-src`, then hits a conflict with the restored files.

Job timings:

| Job | Duration | Install Rust toolchain | Cache rust-src | Cache ARM build outputs | Build ARM binary | Notes |
|-----|----------|------------------------|----------------|-------------------------|------------------|-------|
| host-dev | 37s | 8s | n/a | n/a | n/a | all host checks green |
| fast | 26s | 3s | hit | 3s | failed after 7s | rust-src conflict |
| fast-video | 23s | 4s | hit | 4s | failed after 6s | rust-src conflict |
| device | 27s | 2s | hit | 5s | failed after 5s | rust-src conflict |
| device-video-full | 29s | 2s | hit | 5s | failed after 5s | rust-src conflict |

Interpretation:

- Reject. The narrower `rust-src` cache is unsafe because it restores files
  without rustup metadata. It also failed to avoid the attempted rustup install.
- Remove the `Cache rust-src` workflow step and run a recovery build.

Recovery run after revert:

- Commit: `a20a343`
- Run: `27488896921`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m08s` by run timestamps (`05:00:03Z` to
  `05:02:11Z`).
- Cache behavior: host, ARM target, cross binary, and FFmpeg caches hit primary
  keys. No ARM target caches were saved.
- `rust-src` returned to being downloaded inside `Build ARM binary`.
- Docker apt install layer durations: `30.6s` fast, `22.8s` fast-video,
  `23.7s` device, `24.3s` device-video-full.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 25s | 2s | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache hit |
| fast | 1m40s | 3s | 5s | 1m10s | 2s | 2s | 6s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m55s | 2s | 10s | 1m16s | 0s | 3s | 1s | primary target-cache and FFmpeg hits; binary `9.99 MiB` |
| device | 1m50s | 3s | 4s | 1m27s | 1s | 1s | 1s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 2m02s | 2s | 4s | 1m40s | 1s | 2s | 0s | primary target-cache and FFmpeg hits; binary `9.09 MiB`; tail |

Interpretation:

- Recovery is green and comparable to the accepted workflow shape.
- The `rust-src` cache experiment remains rejected. To make this work safely we
  would need to cache rustup component metadata as well as files, or move to a
  prebuilt toolchain/container strategy; the file-only cache is not acceptable.

## Experiment 38: remove ARM g++ from the cross Docker image

Change:

- Remove `g++-arm-linux-gnueabihf` from `magik-gui/Dockerfile.cross-armv7`.
- Keep native `gcc`, ARM `gcc`, `libc6-dev-armhf-cross`, `libclang-dev`,
  `make`, `pkg-config`, and `ca-certificates`.
- Keep all jobs and artifacts unchanged.

Hypothesis:

- The remaining ARM builds may only need the C cross compiler. The Rust code has
  no local C++ bridge, and the minimal FFmpeg configure currently enables only
  the small static C surface we use.
- If the package is unused, removing it should reduce Docker apt install time
  and image size without affecting the compiled binary.

Risk:

- FFmpeg configure, a transitive build script, or `cross` may still expect
  `arm-linux-gnueabihf-g++` because `CXX_armv7_unknown_linux_gnueabihf` is set.
- Dockerfile changes churn ARM target-cache keys, so the first green run is a
  seed; a warm repeat is required before accepting.

Measure:

- All jobs must pass.
- GLIBC max must remain `GLIBC_2.31`.
- Binary sizes must stay unchanged.
- Compare Docker apt install layer duration and workflow wall time against the
  accepted package set.

Seed result:

- Commit: `62f3c8f`
- Run: `27488995848`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `3m06s` by run timestamps (`05:04:41Z` to
  `05:07:47Z`).
- Cache behavior: Dockerfile change churned ARM target-cache keys and FFmpeg
  cache keys. ARM target caches restored from the previous prefix, then saved
  new primary caches for all four ARM jobs. Video jobs missed the FFmpeg cache
  and rebuilt/saved it.
- Docker apt install layer durations: `22.3s` fast, `23.2s` fast-video,
  `22.3s` device, `23.0s` device-video-full.
- No missing `arm-linux-gnueabihf-g++` failure appeared. All Rust builds,
  video builds, and shared-library checks passed.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Post target cache | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------------------|-------|
| host-dev | 28s | 2s | n/a | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m43s | 4s | 5s | 1m12s | 1s | 2s | 1s | saved new target cache | binary `6.40 MiB` |
| fast-video | 2m33s | 4s | 3s | 2m09s | 0s | 1s | 1s | saved new target cache | FFmpeg cache miss/rebuild; binary `9.99 MiB` |
| device | 2m07s | 3s | 4s | 1m40s | 1s | 1s | 1s | saved new target cache | binary `5.60 MiB` |
| device-video-full | 2m55s | 3s | 3s | 2m33s | 1s | 1s | 0s | saved new target cache | FFmpeg cache miss/rebuild; binary `9.09 MiB`; tail |

Interpretation:

- Build compatibility looks good: ARM `g++` appears unused by the current CI
  matrix.
- Seed timing is not a keep/reject verdict because Dockerfile churned target and
  FFmpeg caches. Run a warm repeat on the same commit.

Warm repeat result:

- Commit: `62f3c8f`
- Run: `27489071585`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m11s` by run timestamps (`05:08:33Z` to
  `05:10:44Z`).
- Cache behavior: host, ARM target, cross binary, and FFmpeg caches hit primary
  keys. No ARM target caches were saved.
- Docker apt install layer durations: `22.0s` fast, `22.2s` fast-video,
  `23.5s` device, `21.8s` device-video-full.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 37s | 10s | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache hit |
| fast | 1m34s | 4s | 3s | 1m13s | 1s | 1s | 1s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m37s | 2s | 4s | 1m18s | 0s | 1s | 1s | primary target-cache and FFmpeg hits; binary `9.99 MiB` |
| device | 1m53s | 2s | 3s | 1m37s | 0s | 1s | 1s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 2m05s | 2s | 4s | 1m42s | 1s | 1s | 1s | primary target-cache and FFmpeg hits; binary `9.09 MiB`; tail |

Interpretation:

- Accept. Removing ARM `g++` is build-safe for the current matrix and preserves
  GLIBC/binary-size outputs.
- Timing impact is small but directionally useful: Docker apt install layers are
  roughly `21.8s`-`23.5s`, compared with the prior accepted image's common
  `22.8s`-`30.6s` range and the immediate recovery run's `22.8s`-`30.6s`
  spread. Wall time remains dominated by Rust build variance.

## Experiment 39: cache rust-src with rustup metadata

Change:

- Add an ARM-matrix cache step for the installed `rust-src` component payload
  plus rustup metadata:
  - `~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/src/rust`
  - `~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/components`
  - `~/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/manifest-rust-src`
- Key it by runner OS and `magik-gui/rust-toolchain.toml`.
- Keep the full build/test matrix unchanged.

Hypothesis:

- Experiment 37 failed because the cache restored only rust-src files, while
  rustup metadata still said the component was absent. Restoring the payload plus
  `components` and `manifest-rust-src` may let rustup/cross see `rust-src` as
  already installed without a download or conflict.
- This should be much smaller than a full `~/.rustup/toolchains/...` cache.

Risk:

- Restoring `components` over the runner-installed toolchain may still confuse
  rustup if the preinstalled stable toolchain changed under the same logical
  `stable` name.
- Cache restore overhead may still be larger than the `rust-src` install it
  avoids.

Measure:

- All jobs must pass.
- The warm repeat must not log `info: downloading component rust-src` during
  `Build ARM binary`.
- GLIBC max and binary sizes must remain unchanged.
- Compare cache restore/save overhead and workflow wall time against the
  accepted Exp 38 warm run.

Seed result:

- Commit: `2a582cf`
- Run: `27489167183`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m30s` by run timestamps (`05:13:19Z` to
  `05:15:49Z`).
- Cache behavior: the new `rust-src-component` cache missed in all ARM jobs.
  `fast` saved a `5.9MB` cache; the other ARM jobs skipped save due key
  reservation contention. ARM target and FFmpeg caches hit primary keys.
- `Build ARM binary` still downloaded `rust-src` in all ARM jobs on the seed
  run, as expected for a cold rust-src-component cache.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.

Job timings:

| Job | Duration | Install Rust toolchain | Cache rust-src component | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|--------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 47s | 10s | n/a | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m25s | 3s | miss/save | 3s | 1m08s | 1s | 1s | 1s | saved rust-src-component cache |
| fast-video | 1m53s | 3s | miss/save skipped | 5s | 1m25s | 1s | 2s | 1s | primary target-cache and FFmpeg hits |
| device | 2m10s | 5s | miss/save skipped | 6s | 1m34s | 1s | 2s | 1s | primary target-cache hit |
| device-video-full | 2m17s | 3s | miss/save skipped | 5s | 1m48s | 1s | 1s | 1s | primary target-cache and FFmpeg hits; tail |

Warm repeat result:

- Commit: `2a582cf`
- Run: `27489227283`
- Event: `workflow_dispatch`
- Conclusion: failure.
- Workflow wall time: `51s` by run timestamps (`05:16:20Z` to `05:17:11Z`).
- The `rust-src-component` cache restored successfully in all ARM jobs, but
  `Build ARM binary` still ran `rustup component add rust-src` and failed.
- Representative error from `fast`:
  `error: failed to install component: 'rust-src', detected conflict:
  'lib/rustlib/src/rust/library/Cargo.lock'`.
- Cache restore overhead was small (`~6MB`), but correctness failed before any
  Rust compilation.

Job timings:

| Job | Duration | Cache rust-src component | Build ARM binary | Notes |
|-----|----------|--------------------------|------------------|-------|
| host-dev | 46s | n/a | n/a | all host checks green |
| fast | 17s | hit | failed after 4s | rust-src conflict |
| fast-video | 22s | hit | failed after 2s | rust-src conflict |
| device | 27s | hit | failed after 4s | rust-src conflict |
| device-video-full | 25s | hit | failed after 2s | rust-src conflict |

Interpretation:

- Reject. Restoring `components` and `manifest-rust-src` alongside the payload is
  still insufficient for rustup/cross to treat `rust-src` as installed.
- A safe rust-src cache would need to cache or preinstall the component through
  rustup itself, not restore partial toolchain files after `rustup show`.
- Remove the `Cache rust-src component` workflow step and run a recovery build.

Recovery run after revert:

- Commit: `8041530`
- Run: `27489269070`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m17s` by run timestamps (`05:18:34Z` to
  `05:20:51Z`).
- Cache behavior: host, ARM target, cross binary, and FFmpeg caches hit primary
  keys. No ARM target caches were saved.
- `rust-src` returned to being downloaded inside `Build ARM binary`.
- Docker apt install layer durations: `20.6s` fast, `24.6s` fast-video,
  `22.8s` device, `22.2s` device-video-full.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 48s | 11s | n/a | n/a | n/a | n/a | n/a | all host checks green; host target cache hit |
| fast | 1m40s | 4s | 4s | 1m17s | 1s | 1s | 2s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m24s | 2s | 3s | 1m05s | 1s | 1s | 1s | primary target-cache and FFmpeg hits; binary `9.99 MiB` |
| device | 1m52s | 3s | 3s | 1m34s | 1s | 1s | 0s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 2m06s | 3s | 9s | 1m34s | 1s | 2s | 1s | primary target-cache and FFmpeg hits; binary `9.09 MiB`; tail |

Interpretation:

- Recovery is green and comparable to Exp 38. The metadata-aware rust-src cache
  remains rejected.

## Experiment 40: install rust-src explicitly in ARM jobs

Change:

- Add an ARM-matrix workflow step after `Install Rust toolchain`:
  `rustup component add rust-src --toolchain stable-x86_64-unknown-linux-gnu`.
- Keep `rust-src` out of `magik-gui/rust-toolchain.toml`, so host-dev does not
  install it.
- Keep all build/test jobs and artifacts unchanged.

Hypothesis:

- Experiment 36 showed declaring `rust-src` in the toolchain file is a poor fit
  because every job, including host-dev, pays the install cost.
- Installing `rust-src` explicitly only in ARM jobs may avoid the repeated
  `cross`/build-step rustup install path without making host-dev slower.

Risk:

- This may simply move the same download from `Build ARM binary` to an earlier
  step, with no wall-time improvement.
- It may add a step without reducing the long Rust compile/link tail.

Measure:

- All jobs must pass.
- `Build ARM binary` should no longer log `info: downloading component rust-src`.
- Compare total wall time and ARM job durations against Exp 38/recovery.

Seed result:

- Commit: `4972828`
- Run: `27489354934`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m32s` by run timestamps (`05:22:56Z` to
  `05:25:28Z`).
- Host-dev remained clean and finished in `25s`.
- `Install rust-src` ran only in ARM jobs and downloaded `rust-src` in about
  `0s`-`1s` by step timestamps.
- `Build ARM binary` no longer logged `info: downloading component rust-src`.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.

Job timings:

| Job | Duration | Install Rust toolchain | Install rust-src | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 25s | 2s | n/a | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m20s | 2s | 1s | 3s | 1m03s | 1s | 1s | 0s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m44s | 3s | 1s | 5s | 1m15s | 1s | 2s | 1s | primary target-cache and FFmpeg hits; binary `9.99 MiB` |
| device | 1m42s | 3s | 1s | 3s | 1m23s | 1s | 1s | 1s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 2m26s | 2s | 1s | 6s | 1m50s | 1s | 2s | 1s | primary target-cache and FFmpeg hits; binary `9.09 MiB`; tail |

Repeat result:

- Commit: `4972828`
- Run: `27489413732`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m27s` by run timestamps (`05:25:59Z` to
  `05:28:26Z`).
- Host-dev remained clean and finished in `23s`.
- `Install rust-src` again ran only in ARM jobs, and `Build ARM binary` did not
  log a `rust-src` download.
- GLIBC guard: all ARM jobs remained at max `GLIBC_2.31`.
- Binary sizes stayed unchanged: `6.40 MiB` fast, `9.99 MiB` fast-video,
  `5.60 MiB` device, `9.09 MiB` device-video-full.

Job timings:

| Job | Duration | Install Rust toolchain | Install rust-src | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 23s | 3s | n/a | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m26s | 3s | 0s | 4s | 1m06s | 0s | 2s | 1s | primary target-cache hit; binary `6.40 MiB` |
| fast-video | 1m20s | 2s | 1s | 4s | 56s | 1s | 2s | 1s | primary target-cache and FFmpeg hits; binary `9.99 MiB` |
| device | 1m54s | 3s | 1s | 3s | 1m36s | 1s | 0s | 1s | primary target-cache hit; binary `5.60 MiB` |
| device-video-full | 2m15s | 5s | 1s | 3s | 1m49s | 1s | 1s | 0s | primary target-cache and FFmpeg hits; binary `9.09 MiB`; tail |

Interpretation:

- Reject. The ARM-only step is technically cleaner than `rust-toolchain.toml`
  because host-dev does not pay for it, and it does prevent the build step from
  downloading `rust-src`.
- It does not reduce wall time. Both runs (`2m32s`, `2m27s`) are slower than the
  accepted Exp 38/recovery range, and the CI tail stayed at `2m15s`-`2m26s`.
- Remove the explicit `Install rust-src` step and run a recovery build.

Recovery result:

- Commit: `8170091`
- Run: `27489509186`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m17s` by run timestamps (`05:30:50Z` to
  `05:33:07Z`).
- Tail job: `device-video-full` at `2m13s`.
- Host-dev stayed fast at `20s`.
- This restores the accepted shape before Exp 40: no explicit `Install rust-src`
  step, no `rust-src` in `rust-toolchain.toml`, and the Dockerfile package trims
  from Exp 7 / Exp 33 / Exp 38 remain.

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 20s | 2s | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m28s | 2s | 3s | 1m08s | 1s | 1s | 0s | primary target-cache hit |
| fast-video | 1m34s | 2s | 5s | 1m06s | 1s | 2s | 1s | primary target-cache and FFmpeg hits |
| device | 1m49s | 3s | 4s | 1m30s | 1s | 1s | 1s | primary target-cache hit |
| device-video-full | 2m13s | 2s | 3s | 1m40s | 1s | 6s | 1s | primary target-cache and FFmpeg hits; tail |

## Experiment 41: remove stale ARM CXX env from cross image

Change:

- Remove `ENV CXX_armv7_unknown_linux_gnueabihf=arm-linux-gnueabihf-g++` from
  `magik-gui/Dockerfile.cross-armv7`.
- Keep `gcc-arm-linux-gnueabihf` and the Rust linker env unchanged.
- Keep all jobs, artifacts, caches, and tests unchanged.

Hypothesis:

- Experiment 38 removed `g++-arm-linux-gnueabihf` and all ARM jobs still passed,
  so this env var is stale.
- Removing it is unlikely to move CI wall time materially, but it makes the
  thinned Docker image internally consistent and confirms no hidden C++ build
  path depends on a configured ARM C++ linker.

Risk:

- A transitive crate or build script may observe `CXX_armv7_unknown_linux_gnueabihf`
  even though the previous run did not invoke the missing binary. If so, CI
  should fail honestly in an ARM build job.

Measure:

- All jobs must pass.
- Compare warm repeat wall time and `device-video-full` tail against recovery
  run `27489509186`.

Seed result:

- Commit: `e143321`
- Run: `27489578912`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `3m02s` by run timestamps (`05:34:14Z` to
  `05:37:16Z`).
- This proved the stale `CXX_armv7_unknown_linux_gnueabihf` env was not needed:
  all ARM jobs compiled, linked, passed the GLIBC guard, and uploaded artifacts.
- Treat this as a seed/noisy run because changing `Dockerfile.cross-armv7`
  changed the target-cache and FFmpeg-cache keys.

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 30s | 8s | n/a | n/a | n/a | n/a | n/a | all host checks green |
| fast | 1m27s | 3s | 3s | 1m05s | 1s | 1s | 1s | target-cache hit/restored despite Dockerfile-key churn |
| fast-video | 2m44s | 2s | 6s | 2m10s | 0s | 2s | 2s | video cache key churn/noisy |
| device | 2m05s | 3s | 4s | 1m38s | 1s | 2s | 1s | target-cache churn/noisy |
| device-video-full | 2m57s | 3s | 4s | 2m36s | 1s | 1s | 0s | video cache key churn/noisy; tail |

Warm repeat result:

- Commit: `e143321`
- Run: `27489654326`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m11s` by run timestamps (`05:37:51Z` to
  `05:40:02Z`).
- Tail job: `device-video-full` at `2m06s`, slightly better than recovery run
  `27489509186` (`2m13s`) and in the accepted warm band.
- This remains a tiny Docker-image thinning/correctness cleanup rather than a
  meaningful wall-time optimization. It removes a stale env that points at a
  binary no longer installed after Exp 38.

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 44s | 13s | n/a | n/a | n/a | n/a | n/a | all host checks green; unrelated slow host toolchain/cache phase |
| fast | 1m37s | 5s | 3s | 1m13s | 1s | 1s | 5s | target-cache hit |
| fast-video | 1m41s | 4s | 3s | 1m18s | 1s | 1s | 1s | target-cache and FFmpeg hits |
| device | 1m55s | 2s | 4s | 1m30s | 1s | 2s | 1s | target-cache hit |
| device-video-full | 2m06s | 2s | 4s | 1m39s | 1s | 1s | 1s | target-cache and FFmpeg hits; tail |

Interpretation:

- Accept. This does not materially speed CI, but it is a successful Docker image
  thinning cleanup with no skipped builds/tests and a warm repeat at `2m11s`.
- The stronger Docker-thinning wins remain Exp 7, Exp 33, and Exp 38: removing
  unused packages from the image while keeping every main ARM build and host
  check intact.

Cache-size observation after Exp 41:

- `gh cache list --ref refs/heads/codex/ci-timing-experiments --limit 100`
  showed ARM target caches at about `225 MiB`-`255 MiB` each, FFmpeg caches at
  about `41 MiB`, and the host target cache at about `177 MiB`.
- Warm restore steps in the accepted runs are usually `3s`-`6s`, so the current
  CI tail is not primarily cache transfer. It is mostly the actual Rust
  build/link step, especially `device-video-full`.
- That makes runner/runtime experiments more promising than further target-cache
  reshaping unless a change reduces what Cargo has to rebuild.

## Experiment 42: run ARM CI on ubuntu-24.04

Change:

- Change only `arm-build.runs-on` from `ubuntu-22.04` to `ubuntu-24.04`.
- Keep the same Ubuntu 20.04 Docker cross image, same `cross` version, same
  cache keys, same matrix, same artifacts, same host-dev job, and same tests.

Hypothesis:

- Newer GitHub-hosted runner images may have better Docker/cache/CPU behavior
  while the project output remains governed by the pinned Ubuntu 20.04 cross
  image and GLIBC guard.
- Because `runner.os` remains `Linux`, existing target/FFmpeg caches should still
  restore.

Risk:

- Docker defaults or runner image contents may change enough to slow builds or
  expose an incompatibility, even though the cross container is pinned.

Measure:

- All jobs must pass.
- GLIBC guard must remain at `GLIBC_2.31`.
- Compare warm repeat wall time and `device-video-full` tail against accepted
  Exp 41 warm run `27489654326` (`2m11s` wall, `2m06s` tail).

Seed result:

- Commit: `42133cf`
- Run: `27489771697`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m13s` by run timestamps (`05:43:35Z` to
  `05:45:48Z`).
- Existing Linux target/FFmpeg caches restored on `ubuntu-24.04`.
- GLIBC guard passed in all ARM jobs.

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 48s | 14s | n/a | n/a | n/a | n/a | n/a | unchanged macOS job; unrelated slow host cache/toolchain phase |
| fast | 1m20s | 2s | 3s | 1m03s | 0s | 2s | 0s | target-cache hit |
| fast-video | 1m30s | 3s | 3s | 1m12s | 1s | 1s | 1s | target-cache and FFmpeg hits |
| device | 1m32s | 3s | 2s | 1m17s | 1s | 1s | 0s | target-cache hit |
| device-video-full | 2m01s | 3s | 3s | 1m44s | 1s | 1s | 1s | target-cache and FFmpeg hits; tail |

Early read:

- Promising. This keeps every build/test/artifact and has the fastest
  `device-video-full` job so far in this run cluster.
- Run at least one repeat before accepting, because host/runner variance is
  visible across adjacent runs.

Repeat result:

- Commit: `42133cf`
- Run: `27489828918`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m16s` by run timestamps (`05:46:19Z` to
  `05:48:35Z`).
- Tail job: `device-video-full` at `2m10s`.
- GLIBC guard passed in all ARM jobs.

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 37s | 10s | n/a | n/a | n/a | n/a | n/a | unchanged macOS job |
| fast | 1m34s | 3s | 4s | 1m11s | 1s | 1s | 1s | target-cache hit |
| fast-video | 1m26s | 2s | 3s | 1m09s | 1s | 1s | 0s | target-cache and FFmpeg hits |
| device | 1m56s | 3s | 5s | 1m28s | 1s | 2s | 1s | target-cache hit |
| device-video-full | 2m10s | 2s | 6s | 1m39s | 1s | 2s | 2s | target-cache and FFmpeg hits; tail |

Interpretation:

- Accept, with variance noted. The seed was `2m13s` wall / `2m01s` tail and the
  repeat was `2m16s` wall / `2m10s` tail. That is competitive with, and often
  better than, the accepted Exp 41 warm run (`2m11s` wall / `2m06s` tail).
- The improvement is not universal (`device` was noisy on repeat), but
  `fast-video` improved in both runs and the runner change does not weaken the
  build surface: same cross image, same tests, same artifacts, same GLIBC guard.
- Keep ARM jobs on `ubuntu-24.04` unless a later main/PR run shows instability.

## Experiment 43: remove ARM cargo registry cache

Change:

- Remove the `Cache cargo registry` step from `arm-build` only.
- Keep the host-dev cargo registry cache.
- Keep ARM target caches, FFmpeg cache, cross binary cache, build matrix, tests,
  shared-library checks, and artifacts unchanged.

Hypothesis:

- On warm ARM runs, the target caches may contain enough compiled output that
  restoring `~/.cargo/registry` and `~/.cargo/git` is not worth the `3s`-`9s`
  per-job restore overhead.

Risk:

- Cargo likely still needs registry and git sources for metadata/fingerprint
  checks, especially the Slint and pprof git dependencies. If it has to
  re-fetch sources, this will regress and should be reverted.

Measure:

- All jobs must pass.
- Compare ARM build-step logs for source downloads.
- Compare warm wall time and `device-video-full` tail against accepted
  `ubuntu-24.04` runs from Exp 42 (`2m13s` / `2m16s` wall, `2m01s` / `2m10s`
  tail).

Result:

- Commit: `a0d5e8d`
- Run: `27489918967`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m16s` by run timestamps (`05:50:56Z` to
  `05:53:12Z`).
- Tail job: `device-video-full` at `2m11s`.
- GLIBC guard passed in all ARM jobs.

Job timings:

| Job | Duration | Install Rust toolchain | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 29s | 8s | n/a | n/a | n/a | n/a | n/a | unchanged macOS job; host cargo cache still enabled |
| fast | 1m28s | 2s | 2s | 1m16s | 1s | 1s | 0s | ARM cargo registry cache removed |
| fast-video | 1m37s | 2s | 3s | 1m23s | 1s | 1s | 1s | ARM cargo registry cache removed |
| device | 1m59s | 2s | 4s | 1m43s | 1s | 1s | 1s | ARM cargo registry cache removed |
| device-video-full | 2m11s | 2s | 2s | 1m58s | 0s | 1s | 1s | ARM cargo registry cache removed; tail |

Interpretation:

- Reject. The workflow stayed green, but removing the explicit registry/git
  restore did not improve wall time and made the slowest build step longer
  (`device-video-full` build `1m58s` versus `1m44s` seed / `1m39s` repeat in
  Exp 42).
- Restore the ARM cargo registry cache. The `3s`-`9s` restore cost is worth
  paying because Cargo still benefits from local registry/git source state even
  when target outputs are cached.

Recovery result:

- Commit: `3b5bf10`
- Run: `27489993431`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m07s` by run timestamps (`05:54:42Z` to
  `05:56:49Z`).
- Tail job: `device-video-full` at `2m02s`.
- GLIBC guard passed in all ARM jobs.

Recovery job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | Cache ARM build outputs | Build ARM binary | Check ARM shared libraries | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|----------------------|-------------------------|------------------|----------------------------|-------------------|---------------------|-------|
| host-dev | 30s | 9s | 4s | n/a | n/a | n/a | n/a | n/a | unchanged macOS job |
| fast | 1m32s | 4s | 3s | 3s | 1m14s | 1s | 1s | 0s | registry cache restored |
| fast-video | 1m41s | 2s | 5s | 5s | 1m12s | 0s | 2s | 1s | registry cache restored |
| device | 1m41s | 2s | 3s | 3s | 1m26s | 1s | 1s | 0s | registry cache restored |
| device-video-full | 2m02s | 3s | 5s | 4s | 1m35s | 1s | 2s | 1s | registry cache restored; tail |

Recovery interpretation:

- The restored cache path is back in the accepted baseline band:
  `2m07s` wall / `2m02s` tail, better than the no-registry-cache run
  (`2m16s` / `2m11s`) and close to the best accepted `ubuntu-24.04` seed.
- Keep the ARM cargo registry cache.

## Experiment 44: remove make from the cross image

Change:

- Remove `make` from `magik-gui/Dockerfile.cross-armv7`.
- Keep every build, test, shared-library check, artifact upload, cache, and
  matrix entry unchanged.

Hypothesis:

- `make` may only be needed when the minimal FFmpeg cache is cold. If the Rust
  cross image can omit it safely, Docker image installs may get slightly
  thinner.

Risk:

- The FFmpeg cache key includes `Dockerfile.cross-armv7`. This change should
  force the video jobs to rebuild minimal FFmpeg with the thinner image. If
  FFmpeg's build system requires `make`, the video jobs should fail and the
  change must be rejected.

Measure:

- All jobs must pass to accept.
- If video jobs fail at the minimal FFmpeg build with `make` missing, record the
  failure as the image-thinning lower bound and restore `make`.

Result:

- Commit: `458a03b`
- Run: `27490137647`
- Event: `workflow_dispatch`
- Conclusion: failure.
- Workflow wall time: `1m57s` by run timestamps (`06:01:50Z` to
  `06:03:47Z`).
- Failure: both video jobs failed while rebuilding minimal FFmpeg after the
  Dockerfile hash changed the FFmpeg cache key.
- Exact failure line from `device-video-full`:
  `bash: line 33: make: command not found`.

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | FFmpeg cache | Cache ARM build outputs | Build ARM binary | Notes |
|-----|----------|------------------------|----------------------|--------------|-------------------------|------------------|-------|
| host-dev | 32s | 3s | 10s | n/a | n/a | n/a | passed |
| fast | 1m46s | 4s | 6s | n/a | 5s | 1m13s | passed; no video/FFmpeg |
| device | 1m50s | 2s | 4s | n/a | 3s | 1m28s | passed; no video/FFmpeg |
| fast-video | 54s | 3s | 3s | miss | 3s | 41s then failed | failed during minimal FFmpeg rebuild |
| device-video-full | 53s | 3s | 3s | miss | 3s | 41s then failed | failed during minimal FFmpeg rebuild |

Interpretation:

- Reject. `make` is required for honest video builds when the minimal FFmpeg
  cache is cold or invalidated. Removing it would create a fragile CI path that
  only works while an old FFmpeg cache happens to exist.
- The non-video ARM jobs passed, confirming the failure is scoped to the
  FFmpeg/video path rather than the Rust linker path.
- Restore `make` in the cross image. Further Docker thinning should focus on
  larger dependencies (`libclang-dev` is required by `ffmpeg-sys-next` bindgen)
  or a prebuilt image/pull strategy rather than deleting this build tool.

Recovery result:

- Commit: `6653951`
- Run: `27490202279`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m09s` by run timestamps (`06:04:52Z` to
  `06:07:01Z`).
- Tail job: `device-video-full` at `2m04s`.
- Minimal FFmpeg cache returned to the accepted Dockerfile hash and hit in both
  video jobs.

Recovery job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | FFmpeg cache | Cache ARM build outputs | Build ARM binary | Notes |
|-----|----------|------------------------|----------------------|--------------|-------------------------|------------------|-------|
| host-dev | 30s | 3s | 8s | n/a | n/a | n/a | passed |
| fast | 1m30s | 2s | 5s | n/a | 5s | 1m05s | passed |
| fast-video | 1m40s | 3s | 4s | 2s hit | 4s | 1m19s | passed |
| device | 1m51s | 3s | 6s | n/a | 5s | 1m25s | passed |
| device-video-full | 2m04s | 3s | 4s | 2s hit | 6s | 1m35s | passed; tail |

Recovery interpretation:

- The PR branch is back to the accepted green band after restoring `make`.
- The Docker image is effectively at a practical package floor for the current
  video build: `make` is needed for cold FFmpeg, `pkg-config` is needed for
  FFmpeg discovery, `libclang-dev` is needed by `ffmpeg-sys-next` bindgen, and
  native/cross GCC packages have already been trimmed to the minimum that links.

## Experiment 45: disable artifact compression on upload-artifact v7

Change:

- Add `compression-level: 0` to both ARM artifact upload steps:
  `Upload ARM binary` and `Upload size history`.
- Keep all builds, tests, shared-library checks, matrix entries, and artifact
  presence unchanged.

Hypothesis:

- The binary artifacts are already small, but v7 still spends roughly `1s`-`2s`
  per ARM job compressing/uploading. Disabling compression may shave a little
  wall time without changing what CI builds or checks.

Risk:

- Larger uploaded artifacts may offset saved CPU time with upload bandwidth.
  Previous upload-compression attempts on older artifact plumbing did not win,
  so this should be accepted only if current v7 timings clearly improve.

Measure:

- All jobs must pass.
- Compare wall time and `Upload ARM binary` / `Upload size history` step timing
  against the current accepted recovery run `27490202279` (`2m09s` wall,
  `2m04s` tail).

Result:

- Commit: `617f362`
- Run: `27490272801`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m15s` by run timestamps (`06:08:27Z` to
  `06:10:42Z`).
- Tail job: `device-video-full` at `2m10s`.

Job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | FFmpeg cache | Cache ARM build outputs | Build ARM binary | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|----------------------|--------------|-------------------------|------------------|-------------------|---------------------|-------|
| host-dev | 38s | 10s | 8s | n/a | n/a | n/a | n/a | n/a | passed; host unchanged |
| fast | 1m25s | 2s | 3s | n/a | 3s | 1m10s | 1s | 0s | passed; faster job but not tail |
| fast-video | 1m27s | 2s | 3s | 1s hit | 3s | 1m11s | 1s | 1s | passed; faster job but not tail |
| device | 2m00s | 5s | 3s | n/a | 3s | 1m40s | 1s | 0s | passed; slower than accepted recovery |
| device-video-full | 2m10s | 3s | 4s | 2s hit | 5s | 1m43s | 3s | 1s | passed; tail, upload binary worsened |

Interpretation:

- Reject. The run stayed green, but wall time and tail regressed versus the
  accepted compressed-upload recovery (`2m15s` / `2m10s` here versus
  `2m09s` / `2m04s` in run `27490202279`).
- The uncompressed binary upload did not help the slowest job:
  `device-video-full` `Upload ARM binary` took `3s`, versus `2s` in the
  compressed baseline.
- Restore the default `upload-artifact@v7` compression setting.

Recovery result:

- Commit: `8eb06c4`
- Run: `27490335008`
- Event: `workflow_dispatch`
- Conclusion: success.
- Workflow wall time: `2m13s` by run timestamps (`06:11:40Z` to
  `06:13:53Z`).
- Tail job: `device-video-full` at `2m06s`.

Recovery job timings:

| Job | Duration | Install Rust toolchain | Cache cargo registry | FFmpeg cache | Cache ARM build outputs | Build ARM binary | Upload ARM binary | Upload size history | Notes |
|-----|----------|------------------------|----------------------|--------------|-------------------------|------------------|-------------------|---------------------|-------|
| host-dev | 51s | 12s | 11s | n/a | n/a | n/a | n/a | n/a | passed; host setup/cache noise |
| fast | 1m20s | 2s | 3s | n/a | 2s | 1m05s | 1s | 0s | passed |
| fast-video | 1m39s | 3s | 5s | 2s hit | 5s | 1m12s | 2s | 1s | passed |
| device | 1m41s | 2s | 3s | n/a | 3s | 1m26s | 1s | 1s | passed |
| device-video-full | 2m06s | 3s | 5s | 3s hit | 4s | 1m39s | 2s | 1s | passed; tail |

Recovery interpretation:

- The branch is green again with default artifact compression.
- Tail recovered from the uncompressed run (`2m10s`) to `2m06s`, consistent
  with rejecting `compression-level: 0`.

## Experiment 46: target-triple-only ARM target cache

Change:

- In the ARM build-output cache, remove `magik-gui/target/${{ matrix.profile }}`
  and cache only
  `magik-gui/target/armv7-unknown-linux-gnueabihf/${{ matrix.profile }}`.
- Give the ARM target cache a new `target-arm-triple-only-*` key prefix so this
  experiment measures a real narrower cache instead of restoring the previous
  broad archive.
- Keep the same host checks, ARM matrix, builds, GLIBC guard, FFmpeg cache,
  cargo registry cache, and artifact uploads.

Hypothesis:

- The top-level `target/${profile}` directory may include host-side proc-macro
  and build-script artifacts that are large to restore and not worth carrying in
  every ARM matrix cache. A target-triple-only cache could reduce restore/save
  size and time.

Risk:

- Cross builds may rely on top-level target artifacts for build scripts,
  proc-macros, Slint generation, or host helper crates. Removing that path may
  make warm builds slower even if the cache archive is smaller.
- The first run uses a new cache family, so it should be treated as a seed. A
  warm repeat is required before deciding.

Measure:

- All jobs must pass and GLIBC guard must remain green.
- Compare cache size/restore time and warm `Build ARM binary` time against the
  default broad-cache recovery run `27490335008` (`2m13s` wall,
  `2m06s` tail).

Seed result:

- Commit: `0c8c3116a81cfd2333b9c2324d47ebe944971a6d`
  (`Test target-triple-only ARM cache`).
- Run: `27490794369`, `workflow_dispatch`, success.
- Workflow wall: `2026-06-14T06:34:36Z` -> `2026-06-14T06:37:54Z`
  (`3m18s` by workflow timestamps; slowest job `3m13s`).
- Job timings:
  - `host-dev`: `20s`
  - `fast-video`: `3m08s`
  - `device`: `3m13s`
  - `fast`: `3m13s`
  - `device-video-full`: `3m01s`
- ARM build steps:
  - `fast-video`: `2m53s`
  - `device`: `2m59s`
  - `fast`: `2m51s`
  - `device-video-full`: `2m46s`
- New target-only cache archive sizes:
  - `fast`: `56.82 MiB`
  - `device`: `55.71 MiB`
  - `fast-video`: `68.87 MiB`
  - `device-video-full`: `67.91 MiB`
- Interpretation: archive size dropped a lot versus the broad target caches
  (`~225-255 MiB`), but the seed run is much slower than the accepted default
  recovery run (`27490335008`, `2m13s` wall / `2m06s` tail). Run one warm repeat
  before rejecting, because this run created the new cache family.

Warm-repeat result:

- Commit: `6782605891b50e4b7e42f747b006826635dbcb60`
  (`Record target-only cache seed timing`).
- Run: `27490890277`, `workflow_dispatch`, success.
- Workflow wall: `2026-06-14T06:39:13Z` -> `2026-06-14T06:43:04Z`
  (`3m51s` by workflow timestamps; slowest job `3m43s`).
- Job timings:
  - `host-dev`: `36s`
  - `fast`: `2m55s`
  - `fast-video`: `3m23s`
  - `device`: `3m35s`
  - `device-video-full`: `3m43s`
- ARM build steps:
  - `fast`: `2m41s`
  - `fast-video`: `2m56s`
  - `device`: `3m10s`
  - `device-video-full`: `3m14s`
- Verdict: reject. The narrower cache archives are much smaller, and restore/save
  overhead is lower, but warm ARM builds are much slower than the broad target
  cache (`27490335008`, `2m06s` tail). The removed top-level
  `magik-gui/target/${profile}` artifacts are useful for cross builds
  (build scripts, proc-macros, generated code, or related host artifacts), so the
  smaller cache loses overall CI time. Restore the broad two-path target cache
  and run recovery.

Recovery result:

- Commit: `aabd7edc64e8de0698be0149e45c1e4564a3383d`
  (`Reject target-only ARM cache`).
- Run: `27490994738`, `workflow_dispatch`, success.
- Workflow wall: `2026-06-14T06:44:08Z` -> `2026-06-14T06:46:13Z`
  (`2m05s` by workflow timestamps; slowest job `2m01s`).
- Job timings:
  - `host-dev`: `26s`
  - `fast`: `1m29s`
  - `fast-video`: `1m40s`
  - `device`: `1m43s`
  - `device-video-full`: `2m01s`
- ARM build steps:
  - `fast`: `1m11s`
  - `fast-video`: `1m20s`
  - `device`: `1m20s`
  - `device-video-full`: `1m38s`
- Recovery confirms the broad two-path target cache is the better final state
  for the PR. It is slightly faster than the previous accepted recovery run
  (`27490335008`, `2m13s` wall / `2m06s` tail), likely normal CI variance plus
  a warm broad-cache restore.

## Experiment 47: Swatinem/rust-cache dependency cache with incremental disabled

Docs read:

- `Swatinem/rust-cache@v2` caches `~/.cargo` and workspace target dependency
  artifacts with Rust/toolchain/environment-aware keys.
- It cleans the cache before saving: unused dependencies, non-dependency build
  products, incremental build artifacts, old mtimes, and pre-existing cargo bin
  entries.
- It does not cache workspace crates by default and automatically sets
  `CARGO_INCREMENTAL=0`, because incremental artifacts are not useful for its
  dependency-only cache model.

Change:

- Replace the host job's manual `actions/cache@v5` Cargo registry and target
  caches with one `Swatinem/rust-cache@v2` step over:
  - `magik-gui -> target`
  - `tools/mister -> target`
- Replace the ARM job's manual Cargo registry and broad ARM target cache with
  one `Swatinem/rust-cache@v2` step over `magik-gui -> target`, keyed by matrix
  name/profile/features.
- Keep the `cross` binary cache, minimal FFmpeg cache, full host checks, ARM
  matrix builds, GLIBC guard, and artifact uploads.

Hypothesis:

- The previous manual target cache is fast but large and stores workspace build
  products. `rust-cache` may restore/save less data and avoid incremental
  overhead while preserving dependency artifacts, potentially improving total
  CI time after a warm run.

Risk:

- This cross build may benefit from cached workspace/build-script/generated
  artifacts that `rust-cache` deliberately removes. If so, it will resemble the
  target-triple-only experiment: smaller caches but slower ARM build steps.
- First run uses a new cache family and must be treated as a seed; a warm repeat
  is required before accepting.

Measure:

- All jobs must pass and GLIBC guard must remain green.
- Compare warm run against the accepted broad-cache final verification
  `27491063325` (`~2m00s` workflow wall, `1m55s` slowest job).

Seed result:

- Commit: `aa9a8c8ca17f0e7b117d5e0d4372fb2a9a15cb42`
  (`Test Swatinem rust-cache`).
- Run: `27491496252`, `workflow_dispatch`, success.
- Workflow wall: `2026-06-14T07:08:19Z` -> `2026-06-14T07:11:58Z`
  (`3m39s` by workflow timestamps; slowest job `3m34s`).
- Job timings:
  - `host-dev`: `1m58s`
  - `fast`: `3m18s`
  - `device`: `3m34s`
  - `fast-video`: `3m28s`
  - `device-video-full`: `3m21s`
- ARM build steps:
  - `fast`: `2m58s`
  - `device`: `3m16s`
  - `fast-video`: `3m07s`
  - `device-video-full`: `2m53s`
- `rust-cache` archive sizes after seed:
  - `host-dev`: `311.50 MiB`
  - `fast`: `417.31 MiB`
  - `device`: `415.94 MiB`
  - `fast-video`: `441.86 MiB`
  - `device-video-full`: `442.03 MiB`
- Interpretation: seed is green but much slower than the accepted broad manual
  cache (`27491063325`, `~2m00s` wall / `1m55s` tail), and the ARM archives are
  larger than the manual broad target caches (`~225-255 MiB`). Run one warm
  repeat before rejecting, because this was the first `rust-cache` save.

Warm-repeat result:

- Commit: `424f6842a5bd07bc86a77ee85038a429662ad9b3`
  (`Record rust-cache seed timing`).
- Run: `27491592717`, `workflow_dispatch`, success.
- Workflow wall: `2026-06-14T07:12:51Z` -> `2026-06-14T07:15:01Z`
  (`2m10s` by workflow timestamps; slowest job `2m05s`).
- Job timings:
  - `host-dev`: `30s`
  - `fast`: `1m27s`
  - `fast-video`: `1m43s`
  - `device`: `1m57s`
  - `device-video-full`: `2m05s`
- ARM build steps:
  - `fast`: `1m12s`
  - `fast-video`: `1m26s`
  - `device`: `1m41s`
  - `device-video-full`: `1m45s`
- Verdict: reject. `Swatinem/rust-cache@v2` is valid and its automatic
  `CARGO_INCREMENTAL=0`/dependency cleanup behaves correctly, but it is not
  faster than the accepted manual cache state (`27491063325`, `~2m00s` wall /
  `1m55s` tail). It also created much larger ARM caches (`~416-442 MiB`) than
  the manual broad target caches (`~225-255 MiB`). Restore the manual Cargo
  registry plus target cache layout and run recovery.

Recovery result:

- Commit: `5387c89ed5e11774a14ece8f7765885db1f51171`
  (`Reject Swatinem rust-cache`).
- Run: `27491665455`, `workflow_dispatch`, success.
- Workflow wall: `2026-06-14T07:16:15Z` -> `2026-06-14T07:18:40Z`
  (`2m25s` by workflow timestamps; slowest job `2m19s`).
- Job timings:
  - `host-dev`: `21s`
  - `fast`: `1m27s`
  - `device`: `1m43s`
  - `fast-video`: `1m45s`
  - `device-video-full`: `2m19s`
- ARM build steps:
  - `fast`: `1m12s`
  - `device`: `1m28s`
  - `fast-video`: `1m19s`
  - `device-video-full`: `1m47s`
- Recovery is green and restores the accepted manual cache layout. The tail is
  slower than the best verification run (`27491063325`, `1m55s` slowest job),
  but the build-step timings are back in the manual-cache range; this looks like
  runner/cache variance rather than a retained workflow regression.

## Experiment 48: BuildKit GitHub Actions cache for the cross helper image

Hypothesis: if `cross` is spending any meaningful time building or checking the
custom armv7 helper Docker image, prebuilding that image with Buildx and the
Docker `type=gha` cache backend may reduce cold-run overhead without changing the
Rust target caches or skipping any jobs.

Change:

- Switch `magik-gui/Cross.toml` from an inline `dockerfile` image definition to
  the named image `cross-custom-rust:armv7-unknown-linux-gnueabihf-b52a5`.
- Teach `magik-gui/build-arm.sh` to build that named image from
  `Dockerfile.cross-armv7` only when it is missing, preserving local/dev
  behavior.
- In CI, set up Buildx and build/load the named image before `Build ARM binary`
  using `docker/build-push-action` with
  `cache-from: type=gha,scope=cross-armv7` and
  `cache-to: type=gha,mode=max,scope=cross-armv7`.

Constraints:

- Keep the existing Cargo registry cache, broad ARM target cache, cross binary
  cache, minimal FFmpeg cache, full ARM matrix, artifacts, and GLIBC guard.
- Do not use a remote registry or skip the Rust build.
- Compare both workflow wall time and ARM build-step time. This may help cold
  image setup, but it may also add overhead to warm jobs if the helper image was
  not a bottleneck.

Seed result:

- Commit: `adf9befbd389023a591aef31579c0a8eca974b88`
  (`Test BuildKit cache for cross image`).
- Run: `27492033712`, `workflow_dispatch`, success.
- Workflow wall: `2026-06-14T07:33:27Z` -> `2026-06-14T07:36:09Z`
  (`2m42s` by workflow timestamps; slowest job `2m36s`).
- Job timings:
  - `host-dev`: `30s`
  - `fast`: `2m07s`
  - `fast-video`: `2m34s`
  - `device-video-full`: `2m34s`
  - `device`: `2m36s`
- Buildx setup plus `Build/load cross helper image` timing:
  - `fast`: `71s`
  - `device`: `69s`
  - `fast-video`: `91s`
  - `device-video-full`: `76s`
- ARM build steps after the image was loaded:
  - `fast`: `37s`
  - `device`: `66s`
  - `fast-video`: `39s`
  - `device-video-full`: `54s`
- Interpretation: valid but much slower than the accepted manual-cache baseline.
  The first run paid heavy Buildx/cache export/load overhead, so run one warm
  repeat before rejecting.

Warm-repeat result:

- Commit: `adf9befbd389023a591aef31579c0a8eca974b88`
  (`Test BuildKit cache for cross image`).
- Run: `27492099061`, `workflow_dispatch`, success.
- Workflow wall: `2026-06-14T07:36:31Z` -> `2026-06-14T07:38:32Z`
  (`2m01s` by workflow timestamps; slowest job `1m57s`).
- Job timings:
  - `host-dev`: `44s`
  - `fast`: `1m26s`
  - `device`: `1m38s`
  - `fast-video`: `1m40s`
  - `device-video-full`: `1m57s`
- Buildx setup plus `Build/load cross helper image` timing:
  - `fast`: `25s`
  - `device`: `23s`
  - `fast-video`: `25s`
  - `device-video-full`: `23s`
- ARM build steps after the image was loaded:
  - `fast`: `42s`
  - `device`: `57s`
  - `fast-video`: `50s`
  - `device-video-full`: `74s`
- Verdict: reject. Warm performance ties the latest restored manual-cache
  verification (`27491758545`, `1m57s` slowest job) but does not beat it, and it
  adds Buildx/build-push-action complexity plus Node 20 deprecation annotations
  from the Docker actions. Restore the simpler `cross` Dockerfile path and keep
  the manual Cargo/target cache layout.
