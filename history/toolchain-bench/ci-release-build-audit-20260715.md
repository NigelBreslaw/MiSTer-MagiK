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
| PR `29447059495` (prebuilt image) | 257 MB hit | 2m24s | 1m33s | 88s |

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

### 2. A slim prebuilt cross image removes repeated construction

The original `Cross.toml` pointed directly to `Dockerfile.cross-armv7`, so each
ephemeral runner pulled Ubuntu 20.04 and installed the toolchain again. The PR
runs spent about 31-33 seconds building/exporting the image; earlier comparable
runs were as high as 37 seconds.

Four clean image/build variants established which packages are actually
required. Sizes below use the same `docker save | gzip -6` method, unlike the
earlier one-off archive measurements:

| Variant | Unpacked | gzip-6 archive | Packages | Image build | Clean production build | Result |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Original Ubuntu image | 814,510,603 B | 236,831,095 B | 167 | 34s | 242s | pass |
| Runtime Clang + host GCC/libc | 534,986,635 B | 178,514,956 B | 157 | 25s | 107s | fail: Clang resource headers missing |
| Runtime + resource headers + host GCC/libc | 609,406,134 B | 196,616,219 B | 161 | 25s | 239s | pass |
| Debian Bullseye toolchain | 1,020,397,653 B | 324,653,677 B | - | 81s | - | rejected: larger |

The winning image is 25.2% smaller unpacked and 17.0% smaller as a controlled
gzip archive. Its clean production compile remained effectively unchanged
(239 seconds versus 242), and it passed FFmpeg compilation, shared-library,
and GLIBC 2.31 gates. Removing either the host libc headers or Clang resource
headers produced real build failures, so further package deletion is not
supported by the current toolchain.

The image is published at
`ghcr.io/nigelbreslaw/mister-magik-cross-armv7:ubuntu20-d047ace4d737` with
manifest digest
`sha256:d199c8f8acc12f8cc0057a2181c96c1685cbbbabad35c1ee61c0ad27bcd5ce4d`.
The content suffix is the first 12 characters of the canonical Dockerfile's
SHA-256. A manual publisher workflow makes replacement deliberate.

Two fresh hosted runners pulled the private GHCR image in 12-13 seconds. The
release path's explicit pull plus build took about 106 seconds, versus 125-131
seconds for the prior build step that constructed the image. This is a measured
net saving of about 19-25 seconds on the relevant path. The full warm release
job fell to 2m24s; the preceding jobs were 3m01s and 3m07s, although 14-15
seconds of those totals was a temporary archive-size measurement.

Use this small cross container, not a full custom runner/VM image. The
toolchain container is the only missing reusable layer and is about 197 MB as
the controlled compressed archive.

Current GitHub billing documentation says public packages are free and Actions
downloads from GitHub Packages have free data transfer. If the image must be
private, shared artifact/package storage is $0.25/GB-month. One measured image
tag is therefore about $0.049/month before included allowance. Keep immutable
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

### 4. Thin LTO is faster, but changes the shipped binary

Manual run `29447425776` built a fresh target directory for four combinations
of LTO mode and codegen-unit count. Run `29448092827` repeated each combination
from its own restored target cache. Every variant used the prebuilt image and
cached FFmpeg, uploaded a Cargo timing report, and passed the shared-library
and GLIBC 2.31 checks.

| Variant | Fresh wrapper / Cargo | Warm wrapper / Cargo | Binary | Versus production |
| --- | ---: | ---: | ---: | --- |
| fat LTO, 1 CGU (production) | 196s / 190s | 92s / 85s | 9,196,012 B | baseline |
| fat LTO, 16 CGUs | 185s / 179s | 87s / 82s | 9,466,348 B | 5-11s faster, 270,336 B larger |
| thin LTO, 1 CGU | 128s / 122s | 66s / 60s | 9,482,788 B | 26-68s faster, 286,776 B larger |
| thin LTO, 16 CGUs | 159s / 154s | 59s / 54s | 10,420,764 B | 33-37s faster, 1,224,752 B larger |

Thin LTO with one CGU was the clean-build winner: 34.7% less wrapper time for
a 3.1% larger binary. It was also 28.3% faster on the warm wrapper path. Thin
LTO with 16 CGUs won the warm measurement by another seven seconds, but was
13.3% larger and had a slower fresh build than thin/1. Fat/16 provided only a
5-11 second improvement for a 2.9% size increase.

These compatibility checks do not prove equivalent on-device performance or
release behaviour. The profile benchmark is evidence for a separate release
tradeoff, not authority to replace the production fat-LTO artifact in this CI
audit.

## Recommendations

1. Keep the 14-day Cargo timing artifact in `rust-arm.yml`. Its uploaded size
   was only 49 KB in the first measured run.
2. Keep the content-versioned GHCR cross image. The measured 12-13 second cold
   pull saves about 19-25 seconds versus constructing it on every runner.
3. Keep the actual production profile in release CI. If a faster release is
   required, thin LTO with one CGU is the only compelling measured candidate:
   its fresh/warm builds were 68/26 seconds faster and its binary 3.1% larger.
   Run the device performance and release gates before changing the shipped
   profile. Do not silently substitute a fast CI-only artifact for the shipped
   binary.
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
