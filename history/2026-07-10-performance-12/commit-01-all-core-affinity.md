# Commit 01 — foreground catalog affinity

Parent: `74ce4c2d0d7fd58a747c2b28f86ea731365f2e35`

## Confirmed causes

- `magik-gui/catalog/src/runtime_thread.rs` represented foreground affinity as
  `Any`, but `apply_affinity` treated that value as a no-op. The catalog worker
  first pinned itself to CPU0, so both foreground catalog roles inherited CPU0.
  The BEFORE sample recorded 145 `library-catalog` samples and 35
  `library-walker` samples on CPU0, with zero on CPU1.
- Starting the cold catalog worker before the first visible copy let promotion
  to all-online contend with launcher startup. The UI had already established
  that SQLite was missing, but the worker also repeated the missing-database
  probe. Cold/empty work is now deferred only until the first visible copy and
  uses the UI's proven cache state; existing database paths retain the worker
  probe and recovery behavior.

## BEFORE

- Label: `P12-C01-AFFINITY-BEFORE-20260710`
- Parent/source: `74ce4c2`; deployed binary checksum:
  `1cbdac69556542a9`
- First frame: 490 ms
- Library scan complete: 92,313 ms
- Library ready: 101,785 ms
- Database saved: 192,608 ms
- Counts: 61,626 discoveries; 44,543 normal files; 2,814 containers;
  16,834 archive entries; 59,228 games; 1,675 coverage-audit rows.
- Policy evidence: both foreground transitions reported `affinity=any` and
  `affinity_status=skipped`; catalog/walker samples ran only on CPU0.

## CANDIDATE

- Label: `P12-C01-AFFINITY-CANDIDATE-20260710`
- Source: `74ce4c2-dirty`; deployed binary checksum: `74e4cffcb6b9d9c3`
- First frame: 96 ms; cold worker started after it at 107 ms.
- Library scan complete: 85,902 ms
- Library ready: 95,174 ms, 6,611 ms / 6.50% faster than BEFORE for
  the combined all-core affinity and first-visible-copy/cache-probe change.
- Database saved: 185,937 ms
- Counts exactly match BEFORE.
- Both foreground transitions report `affinity_status=ok` and
  `allowed_cpus=0-1`.
- Useful work appeared at sample endpoints on both CPUs: catalog samples
  CPU0/CPU1 = 100/37 with user-jiffy deltas 7,020/1,086; walker samples
  CPU0/CPU1 = 12/19 with user-jiffy deltas 120/112. Because the `processor`
  column is the endpoint CPU for each interval, these deltas prove migration
  and useful work while allowed on each core; they are not exact per-core CPU
  accounting for time spent inside a migrated interval.

The first-scan harness returns non-zero because the later-series absolute RAM
and durable-save release gates remain unmet. Commit 01's owned gates—at least
5% better readiness, first frame at most 100 ms, all-online policy evidence,
dual-core useful work, stable counts, and a live launcher—pass.

## REVIEWED

- Label: `P12-C01-AFFINITY-REVIEWED-20260710`
- Candidate source: `74ce4c2-dirty`; deployed binary checksum:
  `38b45014921258f5`
- First frame: 100 ms. The deferred cold worker began at 102 ms, after the
  visible copy.
- Library scan complete: 85,936 ms
- Library ready: 95,121 ms, 6,664 ms / 6.55% faster than BEFORE.
- Database saved: 186,138 ms
- Counts exactly match BEFORE: 61,626 discoveries; 44,543 normal files;
  2,814 containers; 16,834 archive entries; 59,228 games; 1,675 audit rows.
- Both foreground transitions report `affinity_status=ok` and
  `allowed_cpus=0-1`. Endpoint samples show useful work on both cores:
  catalog CPU0/CPU1 samples 97/41 with user-jiffy deltas 6,700/1,182; walker
  CPU0/CPU1 samples 17/13 with user-jiffy deltas 104/84.
- Post-trap safety proof reports every launcher/fault/rebuild arming path as
  `absent`.

## Validation after review

- `scripts/dev-rust check` — pass
- `scripts/dev-rust test` — 231 passed
- `scripts/dev-rust host-tools` — pass, including `profile-first-scan` self-test
- `cargo test --manifest-path magik-gui/catalog/Cargo.toml` — 307 passed
- `cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features`
  — 502 passed
- `cargo clippy --manifest-path magik-gui/catalog/Cargo.toml --all-targets -- -D warnings`
  — pass
- `cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings`
  — pass
- `cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings`
  — pass
- `git diff --check` — pass

## Adversarial review

- Standards reviewer `c01_standards_review`: required parsing actual Linux
  online CPU IDs rather than assuming `0..count`, plus explicit cold-worker
  deferral/cache/lifecycle decision tests.
- Performance reviewer `c01_perf_review`: corrected thread-sample column
  attribution, required explicit post-cleanup device evidence, and required
  combined-change attribution for the 6.50% result.
- Actions: implemented sysfs CPU-list parsing with non-contiguous/range/bounds
  tests; extracted and tested cold/empty/missing/warm/return deferral policy and
  lifecycle decisions; corrected metrics and attribution; made the benchmark
  label dirty for either staged or unstaged source changes; and retained the
  explicit post-cleanup proof.
- Final confirmation: `c01_standards_review` — FINAL APPROVED;
  `c01_perf_review` — FINAL APPROVED.

## Evidence artifacts

- Canonical TSV: `history/toolchain-bench/results-first-scan.tsv`
- BEFORE launcher log:
  `build/first-scan-profiles/P12-C01-AFFINITY-BEFORE-20260710-launcher.log`
- BEFORE thread sample:
  `build/first-scan-profiles/P12-C01-AFFINITY-BEFORE-20260710-first-scan-thread-sample.tsv`
- BEFORE artifact manifest:
  `build/first-scan-profiles/P12-C01-AFFINITY-BEFORE-20260710-artifacts.tsv`
- CANDIDATE launcher log:
  `build/first-scan-profiles/P12-C01-AFFINITY-CANDIDATE-20260710-launcher.log`
- CANDIDATE thread sample:
  `build/first-scan-profiles/P12-C01-AFFINITY-CANDIDATE-20260710-first-scan-thread-sample.tsv`
- CANDIDATE artifact manifest:
  `build/first-scan-profiles/P12-C01-AFFINITY-CANDIDATE-20260710-artifacts.tsv`
- REVIEWED launcher log:
  `build/first-scan-profiles/P12-C01-AFFINITY-REVIEWED-20260710-launcher.log`
- REVIEWED thread sample:
  `build/first-scan-profiles/P12-C01-AFFINITY-REVIEWED-20260710-first-scan-thread-sample.tsv`
- REVIEWED artifact manifest:
  `build/first-scan-profiles/P12-C01-AFFINITY-REVIEWED-20260710-artifacts.tsv`
- REVIEWED post-trap cleanup proof:
  `build/first-scan-profiles/P12-C01-AFFINITY-REVIEWED-20260710-post-cleanup-final.tsv`

Only BEFORE and REVIEWED metrics enter the commit message.
