# CI release-build audit (2026-07-15)

## Scope

This audit investigates why the `Rust ARM / release` GitHub Actions job can
take more than four minutes. The production build is
`magik-gui/build-arm.sh --device`: ARMv7, Cortex-A9 tuning, fat LTO, and one
codegen unit.

Raw Cargo timing HTML remains a CI artifact rather than tracked source. The
workflow uploads `cargo-timings-release` for 14 days, including when the build
fails.

## Measured runs

| Run | ARM target cache | Release job | Build step | Cargo |
| --- | --- | ---: | ---: | ---: |
| Main `29439443160` | miss | 4m25s | 3m49s | 3m07s |
| PR `29443632017` | 257 MB hit | 3m01s | 2m05s | 90.6s |
| PR `29443910399` (docs-only commit) | 257 MB hit | 3m07s | 2m11s | 86.3s |

The PR job totals include a one-off 14-15 second gzip measurement of the cross
image. That measurement was removed after recording the size below.

## Findings

### 1. Fat LTO is the warm critical path

Both warm PR runs restored 389 of 398 Cargo units. The nine dirty units were
the four workspace crates and their build/lib/bin units. The docs-only repeat
did not avoid that work.

Cargo's two reports were consistent:

| Unit | First PR run | Repeat |
| --- | ---: | ---: |
| final `mister-magik-fb` binary | 66.0s | 64.0s |
| `mister-magik-ui` | 23.5s | 21.3s |
| `mister-magik-catalog` | 23.0s | 21.6s |
| `mister-magik-fb` library | 11.0s | 9.8s |

The UI and catalog compile mostly in parallel. The final binary starts after
the UI unit and accounts for roughly three quarters of the 86-91 second warm
critical path. This is consistent with the production profile's fat LTO and
single codegen unit. Reducing it requires a release profile tradeoff, not a
cache or runner-image change.

### 2. The custom cross image is rebuilt on every hosted runner

`Cross.toml` points to `Dockerfile.cross-armv7`, so each ephemeral runner pulls
Ubuntu 20.04 and installs 75 packages. The PR runs downloaded 134 MB of apt
archives, installed 727 MB, and spent about 31-33 seconds building/exporting
the image. Earlier comparable runs were as high as 37 seconds.

The measured image is:

- `814,510,603` bytes unpacked;
- `267,303,666` to `267,319,287` bytes as a gzip-compressed Docker archive.

A versioned public GHCR image keyed by the Dockerfile content should remove
most of the 31-37 second build on hosted runners. This differs from the old
local prebuilt-image experiment: local Docker already had the layers, while a
GitHub-hosted runner starts cold.

Use the small cross container, not a full custom runner/VM image. The toolchain
container is the only missing reusable layer and is about 267 MB compressed.

Current GitHub billing documentation says public packages are free and Actions
downloads from GitHub Packages have free data transfer. If the image must be
private, shared artifact/package storage is $0.25/GB-month. One measured image
tag is therefore about $0.067/month before included allowance. Keep immutable
content-addressed tags but prune superseded tags so storage does not grow
without bound.

### 3. Existing caches are useful but cannot remove the release link

The warm runs restored approximately:

- 89 MB Cargo registry cache;
- 41 MB minimal FFmpeg cache;
- 257 MB ARM target cache.

FFmpeg restored in about three seconds and was already built, so it is not the
current bottleneck. The ARM target cache keeps third-party dependencies fresh,
but a docs-only PR update still produced the same nine dirty workspace units
and 64-second final binary build. Cache tuning alone will not eliminate the
warm 86-91 second Cargo cost.

On a target-cache miss, Cargo rose from 86-91 seconds to 187 seconds. The cache
therefore saves about 96-101 seconds and should remain, even though its key and
restore behavior should continue to be monitored through the timing artifact.

## Recommendations

1. Keep the 14-day Cargo timing artifact in `rust-arm.yml`. Its uploaded size
   was only 49 KB in the first measured run.
2. Publish a versioned public GHCR cross image and A/B its pull time against the
   measured 31-37 second Dockerfile build. Expected gross saving: about half a
   minute; measure the net saving before merging the image switch.
3. Keep the actual production profile in release CI. If a faster release is
   required, separately A/B thin LTO and additional codegen units against
   binary size, shared-library checks, device performance, and release gates.
   Do not silently substitute a fast CI-only artifact for the shipped binary.
4. Treat roughly 1.5 minutes of warm Cargo time as the present production-build
   floor until the fat-LTO profile is deliberately changed. A prebuilt cross
   image can reduce setup time but cannot affect the 64-66 second final unit.

## Sources

- GitHub billing docs source:
  `github/docs:content/billing/concepts/product-billing/github-packages.md`
  (public packages and Actions package transfer are free).
- GitHub billing docs source:
  `github/docs:content/billing/concepts/product-billing/github-actions.md`
  (shared artifacts/package storage is $0.25/GB-month).
- Existing local experiments:
  `history/toolchain-bench/compile-time-experiments-20260609.md`, especially
  experiment 16 (prebuilt amd64 cross image) and the release-profile trials.
