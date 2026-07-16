# Return Catalog Capsule

Date: 2026-07-16

Logical class: performance prerequisite for the production commit train

Benchmark class: performance

## Confirmed cause

The production return path reconstructed the complete navigation projection
before the launcher could restore the selected Arcade game. Phase
instrumentation on the immediate parent showed that file I/O was not the
bottleneck:

- projection file read: 10.5 ms
- LZ4 decompression: 125.7 ms
- binary decode/allocation: 611.7 ms
- catalog construction: 437.3 ms
- total projection load: about 1.197 s

That work kept the display black beyond the 2.0 second gate. A compressed-file
cache was rejected before benchmarking because it could only remove the 10.5 ms
read and introduced cross-layout and metadata-race risks.

## Candidate

Immediate parent:
`b50429dabe12f962e24470caf4090aea5c9e8873`

Candidate source diff SHA-256:
`56bfc8a1ec62a31326d76f1226f7bd9ab9b4a45528b78181d2b0ac0a047ba85a`

The candidate writes a bounded, volatile, one-shot return capsule under `/tmp`.
It contains only the active non-search collection's rows, required system
metadata, and referenced structured launch plans. The capsule is bound to the
device layout, app directory, catalog root, configured roots/path mapping,
durably persisted generation fingerprint, catalog schema/build, and binary
build identity.

The launcher restores the selected collection and game from the capsule, then
hydrates the authoritative full projection and reapplies the saved return
state. The partial catalog cannot exit its restored Arcade collection. It is
discarded if authoritative hydration fails or disconnects, and only a matching
worker `Persisted` generation becomes eligible for a future capsule.

Search-filter return state is deliberately not accelerated because a bounded
capsule cannot reproduce the full search result set safely. It falls back to
the authoritative load. Missing, stale, corrupt, oversized, mismatched, or
failed capsules also fall back. Normal launcher starts remove stale capsules.

The candidate also avoids rebuilding launcher taxonomy when an already usable
summary or capsule has seeded the same navigation state. Fresh or missing
catalog startup still performs the full taxonomy synchronization. The first
human-turbo candidates exposed duplicate or badly placed publication work; the
final form reuses the seeded taxonomy and makes `SyncCatalogBridge` dirty the
existing end-of-frame bridge synchronization instead of rebuilding it twice.

## BEFORE

Command:
`scripts/device-launch-return-smoke.sh --artifacts-dir build/launch-return/RETURN-CAPSULE-B50429DA-BEFORE-FINAL --label RETURN-CAPSULE-B50429DA-BEFORE-FINAL`

The immediate-parent production binary SHA-256 was:
`4095fe1aa570c9e781d2983a7a23c129ec48e3e2ed047c29d5d11dae601229fc`

Representative immediate-parent result:

- game: Air Gallet (Europe), row 17
- total return: 3620 ms
- black interval: 2447 ms
- launch to black: 1173 ms
- result: failed the 3000 ms total and 2000 ms black gates

Raw artifacts:

- `build/launch-return/RETURN-CAPSULE-B50429DA-BEFORE-FINAL/run-context.tsv`
- `build/launch-return/RETURN-CAPSULE-B50429DA-BEFORE-FINAL/report.tsv`
- `build/launch-return/RETURN-CAPSULE-B50429DA-BEFORE-FINAL/run.log`
- `build/launch-return/RETURN-CAPSULE-B50429DA-BEFORE-FINAL/iteration-1-events.jsonl`
- `build/launch-return/RETURN-CAPSULE-B50429DA-BEFORE-FINAL/iteration-1-status.json`
- `build/launch-return/RETURN-CAPSULE-B50429DA-BEFORE-FINAL/iteration-1-main-status.json`

Report SHA-256:
`9a86775347b5cf30b0a13dab962d6286231a8390964796f69b1a11616e74bc3b`

The parent run stopped at the first threshold failure, so it has no successful
completion manifest. Its timing row and failure-state artifacts were retained.

## AFTER

Command:
`scripts/device-launch-return-smoke.sh --artifacts-dir build/launch-return/RETURN-CAPSULE-B50429DA-AFTER-FINAL4 --label RETURN-CAPSULE-B50429DA-AFTER-FINAL4`

The final production binary SHA-256 was:
`1ed97464e8903856d7f2e4d82dbe1d83a5aed8c12e097538ddd84a86bb2519b2`

One production two-game invocation:

| Game | Total return | Black interval | Launch to black | Reveal to input |
| --- | ---: | ---: | ---: | ---: |
| Air Gallet (Europe) | 2320 ms | 1099 ms | 1221 ms | 0 ms |
| Asteroids Deluxe | 2260 ms | 1061 ms | 1199 ms | 0 ms |

Against the comparable first-game parent result:

- total return improved by 1300 ms (35.9%)
- black interval improved by 1348 ms (55.1%)

Both games passed the 3000/2000 ms limits. Arcade selection and selected preview
were restored. The same invocation proved stale return state was ignored and
consumed on a normal launcher start.

Raw artifacts:

- `build/launch-return/RETURN-CAPSULE-B50429DA-AFTER-FINAL4/run-context.tsv`
- `build/launch-return/RETURN-CAPSULE-B50429DA-AFTER-FINAL4/report.tsv`
- `build/launch-return/RETURN-CAPSULE-B50429DA-AFTER-FINAL4/run.log`
- `build/launch-return/RETURN-CAPSULE-B50429DA-AFTER-FINAL4/manifest.sha256`
- `build/launch-return/RETURN-CAPSULE-B50429DA-AFTER-FINAL4/iteration-1-events.jsonl`
- `build/launch-return/RETURN-CAPSULE-B50429DA-AFTER-FINAL4/iteration-1-status.json`
- `build/launch-return/RETURN-CAPSULE-B50429DA-AFTER-FINAL4/iteration-2-events.jsonl`
- `build/launch-return/RETURN-CAPSULE-B50429DA-AFTER-FINAL4/iteration-2-status.json`
- `build/launch-return/RETURN-CAPSULE-B50429DA-AFTER-FINAL4/stale-state-result.tsv`

Report SHA-256:
`ed432037b3c73308f7896ae7285d9eb4a67b7e7fb015420d3f6f53e02d84c562`

Manifest SHA-256:
`21bd180239af06feb8e37e4cfc857ab505ad4e19fec1dfe01960217dd3d9eb18`

The manifest was verified from inside its artifact directory with
`shasum -a 256 -c manifest.sha256`; all 22 retained files passed.

## Runtime regression gate

Command:
`MISTER_DEPLOY_TRANSPORT=ssh scripts/profile-arcade-scroll.sh RETURN-CAPSULE-B50429DA-HUMAN-FINAL --deploy-device --secs 30 --scenario human-turbo-hold --thread-sample`

Final benchmark binary SHA-256:
`e4eb9332d1bdcac33c07709ed227c085f127941af06d4cf0a0af165194bce980`

Result:

- application work over 16.667 ms: 0
- work p99: 6477 us
- maximum application work: 13723 us
- fallback / timeout / error frames: 0 / 0 / 0
- latch deadline misses / visual latch misses: 0 / 0
- FPGA drops: 0
- exact-preview gate: valid, no invalid frames or miss streak
- search ready: 14869 ms, within the 30 second gate
- frame-pacing gate: valid

Raw artifacts share this prefix:
`build/arcade-scroll-profiles/RETURN-CAPSULE-B50429DA-HUMAN-FINAL-*`

The final summary is:
`build/arcade-scroll-profiles/RETURN-CAPSULE-B50429DA-HUMAN-FINAL-arcade-latch-drops.tsv`

## Invalidated candidate runs

No results were averaged. Earlier AFTER runs were retained but invalidated:

- one run found a catalog SQLite read on the UI thread; generation fingerprint
  metadata was moved off the UI thread and the AFTER was rerun
- one run found a catalog publication overrun; duplicate publication work was
  removed and the AFTER was rerun
- one run contained a single unrelated 29 ms Slint render stall
- its nominal confirmation was invalid because a shared ARM target reused a
  stale binary and did not complete search
- a later candidate deployment also detected the immediate-parent binary in
  the shared ARM target; that deployment and run were discarded before the
  final pair, the target was cleaned, and the final candidate binary identity
  was captured in `run-context.tsv`

The build contract was repaired with a clean launcher-scope ARM build. Only the
clean `HUMAN-FINAL` run and the binary-identified `BEFORE-FINAL` /
`AFTER-FINAL4` pair support the acceptance decision.

## Validation

- `scripts/dev-rust test`: 291 passed
- `scripts/dev-rust check`: passed
- targeted UI-runner tests for foreground hydration, durable generation
  eligibility, taxonomy reuse, and partial-load failure: 4 passed
- `cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings`: passed
- `cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings`: passed
- `scripts/device-launch-return-smoke.sh --self-test`: passed
- `bash -n scripts/device-launch-return-smoke.sh`: passed
- production ARM build (`ui`): passed
- clean benchmark ARM build (`ui,bench-tools`): passed
- `git diff --check` for item source and script: passed

Host-tool clippy was run because the production smoke script changed.

The broader UI binary contains a pre-existing failing test,
`summary_projection_is_not_ready_for_arcade_navigation`: the identical test
fails at the identical assertion on the immediate parent. It is not used as a
candidate gate or claimed as a candidate regression.

## Safety and scope

- production code only; no effects or experimental scene work
- volatile `/tmp` state only
- no launcher environment, reboot fault, or persistent arming file
- final device inspection found no launcher env, filesystem-fault, or
  rebuild-on-next-boot arming file
- bounded 8 MiB read/write, parser-time count checks, bounded rows, systems,
  plans, roots, path maps, and strings
- malformed magic, schema, UTF-8, enum, boolean, truncation, trailing bytes, and
  malicious pre-allocation counts are rejected
- atomic temporary-file rename with mode 0600
- authoritative full projection remains the source of truth
- no catalog database or exFAT mutation
- forced authoritative hydration keeps foreground catalog priority

## Review

Final code/safety reviewer:
`review_projection_phase_timing`

- reviewed source diff SHA-256:
  `56bfc8a1ec62a31326d76f1226f7bd9ab9b4a45528b78181d2b0ac0a047ba85a`
- reviewed evidence SHA-256 before recording approval:
  `9082686e2c62d10fb17529c157630fd150ea1152da45d60a2a26071837c9c1c9`
- result: approved with no unresolved actionable findings
- confirmed all five earlier blockers resolved, AFTER manifest valid, metrics
  and binary identities supported, human-turbo clean, and production/device
  safety preserved

Independent benchmark/evidence reviewer:
`design_parallel_projection_decode`

- reviewed the same source and evidence identities
- result: approved with no unresolved benchmark/evidence findings
- independently recomputed timestamps, deltas, thresholds, binary identities,
  selection/preview restoration, stale-state behavior, and human-turbo gates
