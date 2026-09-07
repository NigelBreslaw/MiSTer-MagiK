# Catalog retirement implementation status

Worktree: /private/tmp/mister-magik-catalog-cleanup.
Branch: nigel/catalog-legacy-cleanup. Pinned original: 8dc2908e7.

**Local verification, three-way device equivalence and ordinary smoke passed.**
Cleanup source is committed locally as e25403ce6; no push or PR. Four declarations
remain review-blocked as detailed below. Both worktrees and ignored evidence remain.

## Applied retirement and test ports

The approved scanner/media test retirements and current-adapter ports are applied.
The old SQLite builder, loaders, navigation snapshots, checkpoints, state store,
scanner cache, journal, prepared-target cache, SQL projection and their retired
commands/experiments are disconnected and deleted. Production launch preparation,
current system SQLite search, NavPack, compact metadata, registry publication,
user-state migration and persisted descriptor/version fields remain.

Further review removed unused archive/container types, test-only identity hashing,
orphan fixtures, redundant archive configuration, and the old projection-only
preview archive enumerator plus its newly orphaned helpers. Shared preview lookup
and rendering remain. Shared SQLite helpers moved into sqlite_support; bounded
predecessor deletion remains in the migration owner with symlink/unrelated-data tests.

Current resolver tests cover metadata precedence, explicit identity overrides,
unknown identities and ambiguous canonical families. Six temporary-corpus adapter
regressions cover archives/hidden members, nested/dynamic systems, aliases/shared
cores, disc launchability, symlink exclusion, Arcade prerequisites and AmigaVision.
Those six regressions previously passed against both original and cleaned sources.
Refresh, publication failure/interruption, malformed artifact and migration tests
are retained. Launcher no-I/O assertions now observe current lazy-reader boundaries.

Bounded Rust reference analysis routed to this cleanup worktree; changed preview
code has zero reported diagnostics. Tracked-source searches across feature gates
find no old subsystem module exported. Remaining legacy labels belong to negative
tests, bounded cleanup, preserved format fields or structured progress display.

## Latest local verification

All log paths below are relative to build/catalog-cleanup/ and ignored.
Commands use scripts/cargo and the isolated build/host-tests target, with host
incremental compilation and debug information disabled.

| Check | Result | Log |
| --- | --- | --- |
| Catalog default tests | 288 passed | final-default-tests-3.log |
| Catalog builder tests | 405 unit + 6 fixture tests passed | final-builder-tests-2.log |
| Catalog Clippy, all targets, default | passed | final-default-clippy.log |
| Catalog Clippy, all targets, builder | passed | final-catalog-clippy-2.log |
| App launch preparation | 17 passed | final-launch-preparation-tests.log |
| App command parser | 13 passed | final-command-tests.log |
| Launcher catalog interaction tests | 145 passed | final-launcher-focused-tests.log |
| Host device command parser | 8 passed | final-host-parser-tests.log |
| Host remote helpers | 7 passed | final-host-remote-tests.log |
| Comparator and restoration | 12 passed | final-comparator-tests.log |
| Embedded catalog release boundary | passed | final-release-boundary.log |
| Normal MagiK 2 ARM build | passed | final-arm-build-4.log |

App tests compiled with diagnostics, bench-tools, experiments and magik2 enabled.
An additional broad launcher-name run passed 771 tests but failed eight Slint UI
cases because its platform was initialized on another test thread. All eight passed
isolated-process reruns; the combined invocation is not reported as passing.
Broad cross-platform and visual matrices remain CI-owned, not locally passed.

## Device equivalence evidence

After approved access was configured, fresh original/cleaned/original builds all
passed. Each contains 92 systems, 35,216 games and 350 variants. System metadata,
ordering, complete row/variant fingerprints and all five integrity digests agree.
The original control matches the original baseline: the corpus was stable.
No installed catalog was deleted or replaced; each build used a unique scratch root.

Captures under build/magik2-results/ (each catalog.json):

- Before: 20260907T105647Z-460ac117b705 (51.53 seconds including orchestration).
- After: 20260907T105757Z-ff0d16aa8df5 (48.30 seconds).
- Control: 20260907T110007Z-99aa57ad9774 (48.96 seconds).
Logs: final-before.log, final-after.log, final-control.log. These are correctness
captures, not a performance benchmark. Ordinary smoke passed (1 test, 9 deselected)
in 25.45 seconds: build/magik2-results/20260907T110149Z-cdce85e17dee,
with log final-smoke.log. The cleaned ordinary app was left running.

Matching SHA-256 integrity digests:

- Identity: 2a81f63d853a8a055ca3178fe0d89d7fee2227f517988202d18199411ca5d2bc
- Ordering: 9c5d9faef9ae682d2c41f9212b25c0e47474e6357e2421e9a0d04c0ff3c891b6
- Launch: d8512271eb91e88253e41a62e32a1a670d7065ff1f079c1412d83fb85b3d143e
- Search: af7d0fb6d31c86bba7ba6e9fb425797526c933394d381a28bc068c621f5aaa3c
- Artifact set: a2adffff5371b09230e22bb5b04d5ee7c3684fbf27d49511966c11f568f24e31

The source binary was built before committing the identical source as e25403ce6.
Build: scripts/magik2 build app. Capture: magik2/host/.venv/bin/python -m pytest
magik2/scenarios/test_magik.py::test_catalog_equivalence --magik2-device
--magik2-app magik -q -s, with PYTHONPATH=magik2/host, opt-in equivalence enabled,
approved device access and MISTER_MAGIK2_PREBUILT_ARTIFACT selecting each binary.
After/control also set MISTER_MAGIK2_CATALOG_BASELINE to the fresh before capture.
Smoke: scripts/magik2 check smoke with the cleaned prebuilt binary. Credentials
are deliberately excluded from this report and all committed files.

Pinned original ARM probe SHA-256:
d3a8ff1c273f4109e8cbbaade9b6ae6cb2013d42103c7ffbe3ce0dbd8bc939a6

Cleaned ARM probe SHA-256:
b69d777c82f6dad2356ae70f4b2e0bf56dfe372472842e602ff3f4dfde4ca745

Both binaries: apps/mister/target/armv7-unknown-linux-gnueabihf/release-device-ui-tests/mister-magik-fb
in their respective preserved baseline and cleanup worktrees. Both contain the
same opt-in probe semantics; their probe source differs only in formatting/comment.
The historical 92-system, 35,216-game capture remains at
build/magik2-results/20260907T084507Z-cdc15132fe9a/catalog.json; it is not final acceptance.

## Remaining review block

Automatic review again rejected removal of four obsolete runtime declarations:
MISTER_CATALOG_DURABLE_RESUME, MISTER_CATALOG_READY_SNAPSHOT,
MISTER_LIBRARY_TARGET_ALLOWLIST and MISTER_LIBRARY_SOFTWARE_HASH.
They remain untouched. No implementation readers remain in this worktree;
READY_SNAPSHOT still has an obsolete process-config list entry and qualification
setter/test. Review cited possible production behavior changes. Do not bypass it.
Benchmark-only declarations were removed. Migration path settings remain.

## Physical line accounting

Count physical newlines, including blank lines, comments, tests, manifests and data,
for original Git-tracked catalog files plus new nonignored catalog files. Deleted
files count as zero. Generated logs/captures/build output are excluded.

| Catalog snapshot | All lines | Rust lines |
| --- | ---: | ---: |
| Original 8dc2908e7 | 94,599 | 86,541 |
| Previous unfinished snapshot | 59,679 | 51,717 |
| Current cleanup | 57,737 | 49,775 |
| Net saving from original | 36,862 | 36,766 |

Catalog gross deletions: 37,683; added/extracted/replacement lines: 821;
net saving: 36,862 (39.0%). This latest pass saves another 1,942 catalog lines.
Retained catalog total is 57,737, including the 90-line SQLite utility module and
preserved predecessor-cleanup implementation. Gross deletions are not net savings.

Dedicated validation files account for 527 lines: the 137-line device probe,
59-line I/O observer, 207-line fixture suite, 54-line comparator and 70-line tests.
The scenario adds 57 lines and the app probe hooks add eight more (592 subtotal).
Additional inline test ports and reader instrumentation are included in additions,
not claimed as deleted production code. Whole-tree gross deletion is 39,528 lines,
with 1,698 added: net 37,830, including 968 net savings outside the catalog crate.

## Completion gate

Three-way acceptance and ordinary smoke passed; the verified cleanup is committed.
Only the four declaration removals remain blocked by approval review. Removing
those settings requires resolving that block, not a compatibility implementation.
No push or PR is authorized.
