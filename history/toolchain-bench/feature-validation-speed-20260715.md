# Feature validation acceleration — 2026-07-15

## Outcome

The normal validation loop is now affected-path aware. An ordinary Rust change
fell from a **34.855s** pre-commit median to **1.519s** (**23.0x**, 95.6%
reduction), and a catalog change fell to **6.384s** (**5.46x**, 81.7%
reduction). The complete host assurance gate remains intact and measured
**34.590s**, 0.8% faster than the baseline hook.

The production ARM warm no-op fell from **64.160s** to **5.200s** (**12.34x**,
91.9% reduction). Apple-container ARM checking is operational: the launcher UI
check measured **5.011s** warm, **6.172s** after a Rust touch, and **8.143s**
after a launcher Slint mtime touch.

Production packaging now accepts only one verified immutable
`game-databases-vN` release directory. Raw database arguments and defaults have
been removed. The application distribution workflow downloads the numbered
archive, external manifest, and `SHA256SUMS`; database creation remains confined
to `.github/workflows/game-databases.yml`. Synthetic SQLite bundles remain
temporary test fixtures only.

## Method

- Machine: the same Apple Silicon Mac and 10-CPU, 8 GiB Apple-container builder
  for before and after ARM measurements.
- Toolchain: `rustc 1.97.0`, `cargo 1.97.0`.
- Formal baseline commit: `bb28cd14`, immediately before implementation.
- Sampling: one warm-up plus five measured samples per command.
- Host timing: Hyperfine median/min/max and exit status.
- ARM timing: `scripts/bench-debug-build.sh` wall time, with the same scenario
  and state mutators before and after.
- No MiSTer deployment, reboot, or device contact was used.
- Process checks found no competing Cargo or Apple-container process during the
  formal measured matrices. One later interrupted diagnostic run briefly left a
  container active; that diagnostic was discarded, the process was allowed to
  finish, and the regression gate was rerun successfully while idle.
- The formal affected-path table predates the final review fix that adds the
  deterministic rename/routing self-test to `host-tools-fast`. Those published
  values remain actual measurements of the implemented routing and are not
  silently replaced with projections; the final gate has one additional
  lightweight correctness test.

Raw, machine-readable artifacts are gitignored under `build/`:

- `feature-validation-baseline-host-20260715.json`
- `feature-validation-after-host-stages-20260715.json`
- `feature-validation-after-affected-20260715.json`
- `debug-build-bench.tsv`
- `debug-build-logs/feature-validation-{baseline,after}-20260715-*`

## Required result table

Time saved is `before median - after median`; speedup is `before / after`;
reduction is `time saved / before × 100`. “Historical” rows are context only:
the formal pre-change Apple command failed before timing, so no historical value
is represented as a new actual baseline.

| Benchmark | Before median | After median | Time saved | Speedup | Reduction | Before range | After range | Target | Result |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| Entire pre-commit: ordinary Rust change | 34.855s | 1.519s | 33.336s | 22.95x | 95.6% | 34.732–35.695s | 1.496–1.548s | ≤4s | PASS |
| Entire pre-commit: catalog change | 34.855s | 6.384s | 28.470s | 5.46x | 81.7% | 34.732–35.695s | 6.160–6.481s | <10s | PASS |
| Full host assurance | 34.855s | 34.590s | 0.265s | 1.01x | 0.8% | 34.732–35.695s | 34.046–35.414s | no regression | PASS |
| Production ARM warm no-op | 64.160s | 5.200s | 58.960s | 12.34x | 91.9% | 63.188–84.179s | 4.764–7.999s | <10s | PASS |
| ARM check warm no-op | unavailable: Apple path failed | 5.011s | n/a | n/a | n/a | n/a | 4.627–6.293s | <8s | PASS |
| ARM check after Rust edit | unavailable (historical 3.593s) | 6.172s | n/a (historical -2.579s) | n/a (historical 0.58x) | n/a (historical -71.8%) | historical 3.542–3.682s | 5.989–6.472s | <12s | PASS |
| ARM check after launcher Slint touch | unavailable (historical 7.463s) | 8.143s | n/a (historical -0.680s) | n/a (historical 0.92x) | n/a (historical -9.1%) | historical 7.261–7.761s | 8.026–8.338s | <35s | PASS |

The historical check comparisons use the June cross-backend experiment anchors.
They show that the restored Apple path meets the acceptance budgets but is not
faster than those older cross results. No projected value is presented as an
actual speedup.

## Original hook stages, before and after

Every removed redundant command was still timed directly after implementation.
“Critical path” describes the new automatic hook, not whether the command still
exists manually.

| Stage | Before median | After median | Time saved | Speedup | Reduction | Before range | After range | Critical path now | Status |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |
| Format | 0.744s | 0.755s | -0.011s | 0.99x | -1.4% | 0.742–0.746s | 0.753–0.773s | Always | PASS |
| Host logic tests | 0.290s | 0.300s | -0.010s | 0.97x | -3.5% | 0.283–0.302s | 0.286–0.308s | Affected Rust/catalog | PASS |
| Catalog tests | 6.438s | 6.440s | -0.001s | 1.00x | -0.0% | 6.385–6.495s | 6.397–7.389s | Affected catalog/global | PASS |
| Catalog Clippy | 0.125s | 0.117s | 0.008s | 1.07x | 6.5% | 0.123–0.126s | 0.106–0.120s | Affected catalog/global | PASS |
| `magik-gui` Clippy | 0.244s | 0.223s | 0.021s | 1.09x | 8.6% | 0.239–0.252s | 0.222–0.234s | Affected Rust/catalog/global | PASS |
| Production UI host check | 0.270s | 0.246s | 0.024s | 1.10x | 9.0% | 0.266–0.280s | 0.242–0.252s | Affected Rust/UI/catalog/global | PASS |
| Host logic `cargo check` | 0.197s | 0.185s | 0.012s | 1.06x | 6.1% | 0.193–0.200s | 0.173–0.189s | Removed; same target Clippy | PASS |
| Full host tools | 24.762s | 22.778s | 1.984s | 1.09x | 8.0% | 24.504–25.176s | 22.422–23.307s | CI/manual full only | PASS |
| Agent `cargo check` | 0.062s | 0.049s | 0.013s | 1.26x | 20.6% | 0.062–0.062s | 0.049–0.061s | Removed; same target Clippy | PASS |
| MiSTer tools Clippy | 0.130s | 0.119s | 0.011s | 1.09x | 8.4% | 0.128–0.132s | 0.114–0.123s | Affected tool/global | PASS |
| Agent Clippy | 0.115s | 0.100s | 0.015s | 1.15x | 13.2% | 0.113–0.117s | 0.093–0.107s | Affected agent/global | PASS |
| Complete original 11-stage equivalent | 34.855s | 30.465s | 4.389s | 1.14x | 12.6% | 34.732–35.695s | 30.260–32.021s | Replaced by affected gate | PASS |

The direct catalog-test after range contains one 7.389s outlier; its median is
effectively unchanged. The key improvement is routing, not making that command
artificially appear faster.

## Affected-path scenarios

Before implementation every path paid the same 34.855s hook median. The table
therefore compares each measured scenario with that common formal baseline.

| Affected paths | After median | Range | Time saved | Speedup | Reduction | Exit status | Result |
| --- | ---: | ---: | ---: | ---: | ---: | --- | --- |
| Documentation only | 1.411s | 1.400–1.508s | 33.444s | 24.71x | 95.9% | 5/5 zero | PASS |
| Ordinary `magik-gui/src` | 1.519s | 1.496–1.548s | 33.336s | 22.95x | 95.6% | 5/5 zero | PASS |
| Catalog | 6.384s | 6.160–6.481s | 28.470s | 5.46x | 81.7% | 5/5 zero | PASS |
| Slint UI | 1.546s | 1.459–1.641s | 33.309s | 22.55x | 95.6% | 5/5 zero | PASS |
| `tools/mister` | 1.726s | 1.655–3.325s | 33.129s | 20.20x | 95.0% | 5/5 zero | PASS |
| Workflow/script | 1.654s | 1.609–1.720s | 33.201s | 21.08x | 95.3% | 5/5 zero | PASS |
| Cargo lock/global configuration | 6.807s | 6.702–7.264s | 28.048s | 5.12x | 80.5% | 5/5 zero | PASS |
| Mixed catalog + UI + workflow | 6.793s | 6.764–6.850s | 28.062s | 5.13x | 80.5% | 5/5 zero | PASS |
| Complete `full-host` | 34.590s | 34.046–35.414s | 0.265s | 1.01x | 0.8% | 5/5 zero | PASS |

The `tools/mister` maximum is a single 3.325s outlier; the median remains
1.726s. Additions, modifications, deletions, renames, mixed groups, and global
configuration routing are covered by `scripts/test-validate.sh`.

## ARM scenarios

| Scenario | Before median | After median | Time saved | Speedup | Reduction | Before range | After range | Target/result |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| Production `release-device`, warm no-op | 64.160s | 5.200s | 58.960s | 12.34x | 91.9% | 63.188–84.179s | 4.764–7.999s | <10s PASS |
| Production after Rust binary touch | 61.631s | 64.414s | -2.783s | 0.96x | -4.5% | 61.359–62.986s | 62.386–64.991s | Informational |
| ARM library check, warm no-op | unavailable | 3.896s | n/a | n/a | n/a | n/a | 3.799–4.011s | <8s PASS |
| ARM launcher UI check, warm no-op | unavailable | 5.011s | n/a | n/a | n/a | n/a | 4.627–6.293s | <8s PASS |
| ARM launcher UI check after Rust touch | unavailable | 6.172s | n/a | n/a | n/a | n/a | 5.989–6.472s | <12s PASS |
| ARM launcher UI check after launcher Slint touch | unavailable | 8.143s | n/a | n/a | n/a | n/a | 8.026–8.338s | <35s PASS |
| ARM launcher UI check after shared Slint touch | unavailable | 7.978s | n/a | n/a | n/a | n/a | 7.737–8.710s | <35s PASS |

The production Rust-touch result is slightly slower and remains a real fat-LTO
build. This is not on the new edit-validation critical path and was not given a
speed target: deployment intentionally remains production-equivalent, including
the matching catalog builder. The large actual win is eliminating the no-source
63-second rebuild and restoring compile-only validation.

## Implemented assurance boundaries

### Published game databases

- Only `.github/workflows/game-databases.yml` creates production bundles or
  invokes `mame-metadata-build`.
- Distribution selects the highest numbered immutable release and downloads its
  ZIP, manifest, and `SHA256SUMS` into one directory.
- `scripts/package-distribution.sh` requires
  `--game-databases-release-dir`; raw MAME/HBMAME paths and defaults no longer
  exist.
- The verifier requires exactly the archive named by the numbered manifest,
  compares both external files with their archived copies, validates every
  digest and database sentinel, rejects unsafe/duplicate/unexpected members,
  and extracts only into empty private staging.
- Tests cover valid release extraction, missing/ambiguous releases, external
  mismatch, tampering, undersized and corrupt SQLite, source mismatch, and path
  traversal.

### Validation tiers

- `.githooks/pre-commit` runs only `scripts/validate affected`.
- `--paths-file` makes routing deterministic without modifying the index.
- `host-tools-fast` contains license, shell syntax, static safety, workflow, and
  publication-boundary contracts.
- `host-tools` remains the full package/database/runner/tool assurance command.
- `scripts/validate full-host` is automatic in CI and manual locally; there is
  no pre-push hook.
- Independent affected checks run concurrently with isolated logs, which keeps
  ordinary validation near the slowest required check instead of their sum.

### Stable build metadata and Apple checks

- Build time resolution is explicit non-empty override, checked-out commit time
  in the existing display format, then `unknown`.
- The host wrapper passes the stable value into the container, avoiding Git
  safe-directory limitations inside the image.
- ARM license text now includes the tracked repository root `LICENSE`; wrappers
  no longer create/delete `magik-gui/LICENSE`, the second no-op invalidator.
- `--check` and `--check --lib-only` use the production target/image/cache/FFmpeg
  and Cortex-A9 settings, then exit before binary mirroring, receipts, size
  recording, deployment, or any device operation.
- `scripts/regression-arm-noop.sh` proves two unchanged production builds do not
  compile `mister-magik-fb`, checks the binary embeds the commit timestamp, and
  proves an explicit override invalidates once but not twice.

## Acceptance conclusion

Every specified commit-path and ARM-check budget passes. Full host assurance is
preserved with no regression. Production Rust-touch build time did not improve
and is reported as measured; it remains deliberately outside the fast check
loop. All reported speedups are calculated from measured before/after medians,
and unavailable pre-change Apple checks remain marked unavailable rather than
being converted into projected speedups.
