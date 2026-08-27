# Specialised versus generic fast-source A/B

Date: 2026-08-27

The experiment ran twice on the real MiSTer after supervised reboots. The first
sample ran the specialised adapters first; the second ran the generic walker
first. Only the first adapter in each sample is used for the authoritative cold
comparison. The second adapter exposes warm-cache sensitivity.

Evidence: `build/agent-benchmarks/fast-refresh/source-specialized-vs-generic-ab.json`

## Cold results

| System | Specialised | Generic | Result |
|---|---:|---:|---|
| Arcade | 3.914 s | 8.633 s | Specialised 2.21x faster |
| Amiga | 0.011 s | 0.155 s | Specialised 14.3x faster, but 1 versus 3 rows |
| DOS | 2.441 s | 0.247 s | Generic 9.90x faster, but includes 5 invalid rows |
| X68000 | 1.463 s | 0.264 s | Generic 5.53x faster; launch-reference parity failed |
| C64 | 3.510 s | 13.398 s | Specialised 3.82x faster |
| **Five-system total** | **11.381 s** | **22.768 s** | **Specialised 2.00x faster** |

## Row parity

| System | Specialised | Generic | Common | Missing | Extra | Exact |
|---|---:|---:|---:|---:|---:|---|
| Arcade | 1,189 | 19,978 | 1,185 | 4 | 18,793 | No |
| Amiga | 1 | 3 | 1 | 0 | 2 | No |
| DOS | 300 | 305 | 300 | 0 | 5 | No |
| X68000 | 545 | 545 | 543 | 2 | 2 | No |
| C64 | 2,294 | 2,287 | 2,287 | 7 | 0 | No |

The generic baseline recursively walked the live source trees using launch
profiles and extension rules. It loaded no catalog snapshot or precomputed file
inventory. Its Arcade result demonstrates why timing alone is not sufficient:
it accepted organised duplicates and MRAs lacking the specialised ROM/core
validity contract. DOS was faster because it skipped the per-MGL launch
validation that rejected five unusable launchers. X68000 likewise failed exact
launch-reference parity.

The defensible conclusion is limited: the specialised group is twice as fast
overall on this installation, and the Arcade and C64 prepared knowledge gives a
large cold win. It is not true that every specialised adapter is faster. DOS
and X68000 deliberately spend more time validating launchability, while the
current Amiga installation is too incomplete for a meaningful speed claim.

## Retained source optimisations

Evidence:
`build/agent-benchmarks/fast-refresh/source-specialized-vs-generic-optimized-ab.json`

Two measured changes were retained:

- Known 0MHz v0.04 launchers use the checked-in release manifest and a batched
  payload-presence index. Of 305 installed launchers, 304 matched the release
  helper and one used full MGL fallback validation. Cold DOS time fell from
  2.441 s to 1.404 s, a 42.5% reduction. The helper accepted four additional
  release-known launchers whose payloads were present, leaving one invalid
  custom or unmatched launcher rejected.
- Neon68K MGL validation uses two bounded Cortex-A9 lanes after file discovery.
  Cold X68000 time fell from 1.463 s to 1.279 s, a 12.6% reduction, with the
  same 545 specialised launch references and the same generic parity result.

Arcade and C64 were unchanged. Their run-to-run cold variation dominates the
five-system total, so the per-adapter timings above are the useful evidence for
these targeted changes.

## Rejected 0MHz experiment

Evidence:
`build/agent-benchmarks/fast-refresh/source-specialized-vs-generic-direct-0mhz-ab.json`

Replacing the batched payload index with one direct filesystem check per
manifest payload produced 1.442 s cold versus 1.404 s for the batched version.
It was faster only with warm metadata. The direct-check variant was therefore
removed; the retained implementation groups expected payloads by directory and
enumerates each directory once.
