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
