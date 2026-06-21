# CORR-01 Virtual Launch Cache Filename Fix

Date: 2026-06-21

## Summary

Virtual launch `.mgl` cache paths now use bounded filenames:

```text
virtual-{readable-slug}-{fnv128-launch-ref-hash}.mgl
```

The readable slug is capped at 80 ASCII bytes and the 128-bit hash is computed
from the full `launch_ref`, so path-derived virtual launch refs no longer become
unbounded filesystem basenames.

## Verification

- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features virtual_launch`
  - Passed: long path-derived refs, slug collisions, empty slug fallback, warm cache, and stale cache refresh.
- `cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings`
  - Passed.
- `magik-gui/build-arm.sh --device`
  - Built `mister-magik-fb` release-device binary, 6,164,188 bytes.
- `scripts/deploy-rust.sh`
  - Deployed the fixed binary to `/media/fat/mister-magik/mister-magik-fb`.
- Device `library-refresh`
  - Reported `virtual_launch_total=1040`, `virtual_launch_unchanged=1040`, `virtual_launch_errors=0`.
- `scripts/profile-launch-prep.sh FIX-LAUNCH-COLD --replace-label --scenario cold --iterations 2`
  - Reported `count=24`, `errors=0`.
- Exact long GBA launch ref from the review:
  - Prepared successfully as `/media/fat/mister-magik/launch-cache/virtual-magik-plan-payload-media-fat-games-gba-crash-spyro-superpack-spyro-orange-the-co-9c19c14d8f8985bf5dc57e71c3cf1b7c.mgl`.
- `scripts/mister doctor --json`
  - Reported `No obvious launcher/display problems found`.
