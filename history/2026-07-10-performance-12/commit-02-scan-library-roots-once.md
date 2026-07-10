# Commit 02: scan library roots once

## Confirmed cause

The parent performs a separate depth-two payload-fact traversal while resolving
active profiles, then walks the selected library directories again for catalog
classification. On the Cortex-A9/exFAT device, the parent recorded
`active_profiles=45.364s` before a `37.827s` walker.

This change introduces `CatalogScanPlan`, retains installed-core and top-level
game-directory facts in `LibraryScan`, resolves runtime profiles from facts
collected by the catalog walker, and reuses those facts for the coverage audit.
Empty directories remain installation-local evidence: they produce no games on
that SD card but are never hard-coded as unsupported for another installation.

## Before

- Label: `P12-C02-SCANPLAN-BEFORE-20260710`
- Parent: `994aa44a1d64ae7ad81984d188354a6f17fe9478`
- Binary checksum: `38b45014921258f5`
- First frame: 180ms
- `library_scan_complete`: 85.708s
- `library_ready`: 94.693s
- `library_db_saved`: 183.127s
- Counts: 61,626 discoveries; 44,543 normal files; 2,814 containers;
  16,834 archive entries; 59,228 games; 1,675 audit rows.

## Candidate retained by user direction

- Label: `P12-C02-SCANPLAN-CANDIDATE-20260710`
- Source: `994aa44-dirty`
- Binary checksum: `d2eeaad1d04bce2d`
- First frame: 96ms
- `library_scan_complete`: 85.829s (0.14% slower)
- `library_ready`: 94.968s (0.29% slower)
- `library_db_saved`: 179.744s (1.85% faster)
- `active_profiles`: 36.253ms of fact-only derivation
- Counts exactly match the parent.

The planned 25% scan-complete improvement did not materialize: eliminating the
pre-walk also eliminated its exFAT metadata warming, and the cold walker grew to
73.143s. The user explicitly directed that the fact-reuse architecture be kept
and committed despite this failed threshold. No performance improvement is
claimed for `library_scan_complete` or `library_ready`.

## Validation and review

Pending staged standards/spec reviews and the final `REVIEWED` hardware run.

## Evidence artifacts

- Canonical TSV: `history/toolchain-bench/results-first-scan.tsv`
- BEFORE log: `build/first-scan-profiles/P12-C02-SCANPLAN-BEFORE-20260710-launcher.log`
- BEFORE thread samples: `build/first-scan-profiles/P12-C02-SCANPLAN-BEFORE-20260710-first-scan-thread-sample.tsv`
- CANDIDATE log: `build/first-scan-profiles/P12-C02-SCANPLAN-CANDIDATE-20260710-launcher.log`
- CANDIDATE thread samples: `build/first-scan-profiles/P12-C02-SCANPLAN-CANDIDATE-20260710-first-scan-thread-sample.tsv`
