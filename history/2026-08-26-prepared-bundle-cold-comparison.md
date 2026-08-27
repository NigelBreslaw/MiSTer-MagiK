# Prepared bundle catalog helper: cold hardware comparison

Date: 2026-08-26

Branch: `nigel/arcade-catalog-prototype`

Device: production MiSTer SD-card corpus on exFAT

## Result

Prepared bundle helper v2 is correct and modestly faster for a whole-card Catalog V3 build. It removes substantial prepared-target work, but whole-card publication and unrelated system scans now dominate.

| Trusted fresh metric | Existing scanner | Helper v2 | Change |
| --- | ---: | ---: | ---: |
| Catalog scan | 112.118 s | 108.366 s | **3.752 s faster (3.35%)** |
| Execution pipeline | 107.573 s | 88.034 s | **19.539 s faster (18.16%)** |
| Authoritative catalog prepared | 142.970 s | 140.755 s | **2.215 s faster (1.55%)** |
| Files walked | 72,990 | 38,482 | **34,508 fewer (47.28%)** |
| Candidates classified | 54,186 | 34,688 | **19,498 fewer (35.98%)** |

Both trusted samples were fresh builds after a verified supervised reboot. The isolated catalog outputs were removed before each sample, `sync` completed, and Linux page caches were dropped. Warm and forced-rebuild legs are excluded.

## Exactness

Both builds produced 40,013 games across 69 systems. All Catalog V3 identity fields matched:

- fingerprint: `76b1b9a0f44ddc65`
- identity SHA-256: `0cdea8b6a2ea598c4175f92b15bbada2a7b788ad108d6e008fdcb1ebc9611b56`
- ordering SHA-256: `bf5bda0a0218a75b0452d223764ac3c9a1bc373dbab81f49ba7ce0bfc34bd638`
- launch SHA-256: `3f7d22b5c0b50f11fe2c00a0b7e5dff5cfda504abbf5e6aded5382b4320a5a32`
- search SHA-256: `7c4f5e4a20a0f730623cb0691401c00e0aefb73e452eb97bec6802fedadd9ac7`
- artifact-set SHA-256: `844195ea432558bce9ec2e6cbf675d97ef529474aa58da1e46064049c8246ad0`

Helper v2 activated four exact prepared targets:

- Neon68K: 272 discoveries
- AmigaVision/Amiga target: 1,563 discoveries
- 0MHz: 305 discoveries
- C64 target containing OneLoad64: 18,851 discoveries

New or removed files change a recorded directory generation and reject only that target. Changed launcher metadata or external/archive payload receipts also reject the helper. Rejected targets use the normal scanner, so custom collections and new beta content remain visible.

## Evidence

- Existing scanner: `build/agent-benchmarks/catalog-full-build-rebuild/1787742734/fresh-launcher.log`
- Existing catalog identity: `build/agent-benchmarks/catalog-full-build-rebuild/1787742734/fresh-catalog-inspect.tsv`
- Helper v2: `build/agent-benchmarks/catalog-full-build-rebuild/1787744816/fresh-launcher.log`
- Helper v2 catalog identity: `build/agent-benchmarks/catalog-full-build-rebuild/1787744816/fresh-catalog-inspect.tsv`

The enclosing benchmark command reports failure in a later forced-rebuild leg because its updater-index evidence assertion expects a fresh prefetch record. This does not invalidate either retained fresh sample; both fresh inspect files are valid and exact. That harness assertion should be repaired separately.

## Remaining bottleneck

Helper validation and output decode still account for roughly 15.96 seconds, while skipping the four target walks saves roughly 19.54 seconds. The next optimization should split the mixed C64 target so the helper stores only OneLoad64 rows and the walker prunes only that subtree. That will shrink the 12.6 MB helper output and leave custom C64 content on the normal path without validating the whole target.
