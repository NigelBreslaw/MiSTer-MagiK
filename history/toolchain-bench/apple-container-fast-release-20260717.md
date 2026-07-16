# Apple-container optimized fast release - 2026-07-17

Goal: provide an explicit optimized daily-deploy artifact without changing the
fat-LTO production `release-device` profile or any existing command default.

## Baseline and experiment

Apple container builder: 10 CPUs, 8 GiB. Workload: touch
`magik-gui/src/ui_runner.rs`, then build the current UI-enabled ARM binary.
Each result is the median of three warmed samples.

| Profile | LTO | CGUs | Wall | Cargo | Binary |
| --- | --- | ---: | ---: | ---: | ---: |
| `release-device` | fat | 1 | 64.4 s | 60.0 s | 9,974,284 B |
| `release` | thin | 16 | 26.8 s | 23.1 s | 11,305,524 B |

The thin-LTO profile was 58% faster and 13.3% larger. It retains optimization
level 3, stripping, panic-abort, Cortex-A9 tuning, and LTO. Fat LTO remains the
production/distribution profile because it provides the strongest whole-program
optimization and the smallest measured artifact.

Warm no-op `release-device` builds were 4.9 s wall / 1.6 s Cargo, showing that
container startup and artifact mirroring are secondary to final optimized
codegen. Changing the persistent builder from 2 CPUs/2 GiB to 10 CPUs/8 GiB
did not materially change the single-CGU incremental link, but remains useful
for clean dependency compilation.

## Accepted interface and policy

- `magik-gui/build-arm.sh --fast` builds `release` and defaults to launcher UI
  scope unless the caller explicitly selects another scope.
- `scripts/deploy-rust.sh --fast` deploys that artifact with normal build
  receipt and identity verification.
- Bare builds, bare deploys, CI, packaging, release, and benchmark publication
  continue to use `release-device`.
- Apple builds warn about an undersized builder and print restart instructions;
  they never restart the shared builder automatically.

Raw samples remain in the ignored `build/debug-build-bench.tsv`.

## Implementation qualification

The implemented `build-ui-fast` path measured 28.3 s median wall / 24.2 s
Cargo versus 66.2 s / 62.0 s for the same-run `build-ui-device` baseline: a
57.3% wall-time reduction. The `release` binary was 11,305,524 bytes versus
9,974,284 bytes for `release-device` (+13.3%).

A verified `release` + `ui,bench-tools` binary was deployed for the supervised
30-second `human-turbo-hold` gate. Its runtime gates passed: p99 work 6.916 ms,
zero work frames over 16.667 ms, zero wall frames over 33.334 ms, and zero
fallback, timeout, error, unknown-vsync, or miss-streak frames. The composite
wrapper returned invalid because the already-cached search index emitted no
build-start/ready events. A forced catalog-refresh retry was also invalid: it
changed the startup workload and missed the 100 ms first-navigation presentation
gate at 1,281 ms. Neither invalid setup is recorded as a full composite pass.
The production `release-device` binary was rebuilt and restored afterward with
a verified matching deployed hash.
