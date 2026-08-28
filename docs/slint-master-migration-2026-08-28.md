# Slint master migration evidence

Measurement date: 2026-08-28. The candidate pins Slint revision
`72cf74306784e7d15374639ef110c1bb85c21cb0` (master, reported as 1.18.0).
Measurements ran on Apple Silicon (`arm64`, macOS 26.6.2) with Rust/Cargo
1.98.0 using the canonical `magik-release-device-arm-production` campaign.

## Compile time and binary size

| Revision | Cold build | No-op median | Edit median | Binary size |
| --- | ---: | ---: | ---: | ---: |
| Slint 1.17.1 baseline (`bfb05403b`) | 185.610 s | 53.510 s | 49.289 s | 25,853,992 bytes |
| Slint master pin (`2dc8c0bcc`) | 191.260 s | 44.254 s | 47.334 s | 23,002,764 bytes |
| Master + compiler feature trim (`cf0905f5a`) | 136.502 s | 41.173 s | 45.475 s | 23,003,788 bytes |

Relative to the baseline, the final candidate is:

- cold build: 26.892% faster;
- no-op rebuild: 23.055% faster;
- selected edit rebuild: 7.738% faster;
- binary: 2,850,204 bytes smaller (11.024%).

The compiler feature trim accounts for the cold-build improvement. Its binary
size differs from the untrimmed master pin by only 1,024 bytes, so the binary
reduction is attributable to the Slint master dependency/generated-code update
and lockfile changes rather than `slint-build` feature trimming alone.

Binary hashes:

```text
baseline  b0402ef7432c1762495d97920c2e2fae109d35936bead4fd3b0b696dd9254985
master    2b0072bff86165b27150e315e608e06c617bf5e07d08a73ed6895be782cd5df8
trim      6c874aff096be73e98f3bf2762e5090fcafc17e495bc719b3ac2605fce7b99be
```

## UI validation

The macOS preview rendered and compared all 18 launcher scenes successfully.
One intentional Slint master renderer difference was accepted and recorded in
the separate baseline commit: the bottom border of the final row in
`crt-240p-controller-setup` is now drawn.

No device runtime benchmark was recorded. The typed delivery workflow stopped
before device staging because this isolated branch has no upstream, and the
branch was not pushed. Device scenarios (`settled-composition`,
`bridge-model-churn`, and `launcher-response`) should be run after an attended
delivery from a qualified upstream branch.
