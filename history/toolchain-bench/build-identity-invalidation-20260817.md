# Build-identity crate invalidation experiment

This experiment measures a history-only commit after the build-identity split
(`c4ba0e5bf`). The canonical `release-device` target is warmed before the
history commit, then rebuilt after the Git revision changes without any
frontend source edits.

Baseline warm-up build before the history-only commit:

- target: `magik-release-device-arm-production`
- profile: canonical `release-device` (thin LTO, 32 CGUs)
- target directory: `/private/tmp/mister-magik-campaign/identity-target`
- cold build wall time: **172.99 s**

The next commit changes only this evidence file, so its build time measures
whether the isolated metadata crate avoids invalidating the large frontend
crate when a delivery commit changes identity metadata.

Post-history-commit result:

- build wall time: **51.117 s**
- Cargo reported `release-device` completion in **47.04 s**
- Cargo recompiled both `mister-magik-build-identity` and `mister-magik-fb`

The split is therefore rejected as a compile-time optimization: dependency
metadata changes still invalidate the frontend crate. The implementation is
reverted in the following commits; this evidence remains as a negative result
for future crate-boundary work.
