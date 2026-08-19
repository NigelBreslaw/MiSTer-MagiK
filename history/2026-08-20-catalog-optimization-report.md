# Catalog build optimization report

Date: 2026-08-20

Qualified implementation: `dd6e5136884ff47143a2ce75accaf3d757904e67`

Evidence manifest: `history/2026-08-20-catalog-optimization-evidence.json`

## Outcome

The final whole-card qualification passed with the normal configured library
sources on exFAT. All three legs produced the same logical catalog fingerprint,
69 systems, and 39,791 games. Required scan, projection, reconciliation,
persist, and completion evidence was present.

| Operation | Reference | Final | Change |
| --- | ---: | ---: | ---: |
| Full clean build, harness wall | 272.112 s | 231.120 s | -40.992 s (-15.1%) |
| Equivalently warm full clean build | n/a | 247.302 s | observation only |
| Forced unchanged rebuild, harness wall | 169.457 s | 116.592 s | -52.865 s (-31.2%) |
| First clean build, device builder | n/a | 165.759 s | observation only |
| Warm clean build, device builder | n/a | 167.083 s | observation only |
| Forced rebuild, device builder | n/a | 98.883 s | observation only |

The harness wall measurement includes launcher restart, status polling, catalog
inspection, and evidence retention. The device-builder timing is the catalog
builder's own persisted event. The harness wall result remains the authority
for user-visible completion.

The final clean-build midpoint at the device builder was 166.421 s. The
immediately preceding pre-pipeline observations were 215.419 s and 211.924 s,
a midpoint reduction of 47.251 s (22.1%). Those intermediate observations are
diagnostic rather than the qualified reference because their three-leg run was
invalidated by the old no-op generation predicate.

## Retained implementation

1. `9f04f4ee8` canonicalizes preview availability during initial projection, so
   unchanged preview reconciliation does not create a repair generation.
2. `d990313a6` gives target-output state unique schema ownership. The state is
   retained for interrupted fresh-build recovery, but ordinary rebuild replay
   is disabled after its measured loss.
3. `157c6d600`, `38dc47d08`, and `28dc4c38f` add persisted-search phase evidence,
   reuse normalized search fields, and replace ordered-map accumulation with a
   hash accumulator followed by deterministic lexical sorting.
4. `ce4762a54` and `8f130fb7a` enable the bounded fresh shard construction and
   serialized publication pipeline even when the durable journal is active.
   Final clean legs used two workers and overlapped 18.601 s and 19.354 s.
5. `c79f7ddc8` renames generation-private artifacts already staged beneath the
   exFAT catalog root. The qualified workload used tmpfs staging, so this route
   was correctly reported as unexercised rather than credited with the win.
6. `079b5b6e9` replaces two full target-cache copies with same-directory,
   synchronized renames. This removed the large cache-copy tail from clean
   completion and preserves restart recovery through the mutable journal.
7. `ff0959a36` and `29486b15c` make unchanged rebuild completion and its zero-work
   reconciliation evidence explicit.
8. `dd6e51368` disables target-output replay for rebuild operations because the
   measured exFAT decode path was slower than a normal scan.

## Rejected experiment

Target-output replay was rejected, not hidden. It successfully reused 159 of
160 targets and reduced the second execution walk to Arcade, but opening and
decoding the cache cost 123.065 s and validation cost another 38.874 s. The
result was a 204.644 s rebuild, 35.187 s slower than the 169.457 s reference.
With replay disabled, rebuild fell to 116.592 s.

The failure is representational: a large SQLite database of JSON target outputs
is a poor cold-read format on this exFAT card. It does not disprove target-level
reuse with a compact sequential format.

## Phase findings

- Full clean scan remains about 66-67 s of device work.
- Fresh shard construction/publication takes about 44-45 s wall. The pipeline
  overlaps roughly 19 s, but concurrent copy/hash expands publication to about
  25-27 s.
- Every clean build still publishes 73,313,125 artifact bytes from tmpfs and
  spends about 24.8-25.3 s in copy/hash during the final qualified run.
- The final unchanged rebuild spends 65.150 s in scanning, followed by about
  17.451 s preparing the canonical catalog, 7.625 s proving projection is
  unchanged, and 5.268 s writing scanner cache.
- The unchanged rebuild now emits zero shard work and retains generation 1.
- First-observed Arcade visibility was 26.414 s versus the old 9.437 s
  reference. This campaign deliberately optimized complete catalog work after
  the user removed scroll performance from scope; the visibility regression is
  recorded and must not be described as a retained startup win.

## Next high-impact experiments

### 1. Short-circuit unchanged rebuild before catalog construction

After the final scan, rebuild still spends roughly 33.6 s before persistence
completes even though reconciliation proves that no shard changed. Persist a
versioned semantic scan fingerprint with catalog state, compute the same digest
from classified scan output, and bypass catalog SQLite/search construction,
projection comparison, and scanner-cache rewrite when it matches.

- Bounded experiment: real Arcade plus actual SNES and C64 roots; whole-card
  confirmation only after the bounded result passes.
- Before/after metric: scan-complete to builder-persisted wall time and complete
  rebuild wall time.
- Correctness cases: add, remove, rename, same-size content change affecting
  MRA/MGL semantics, archive member change, profile/taxonomy change, corrupt
  state, and interrupted publication.
- Reject if the tail does not fall by at least 15 s or any logical fingerprint
  differs.
- Conservative whole-card opportunity: 20-30 s from unchanged rebuild.

### 2. Use a compact sequential target cache, not SQLite JSON

Store a small indexed header containing target ordinal, versioned signature,
output offset, compressed length, and affected-system digest, followed by
independently compressed canonical target-output frames. Validate signatures
first and decode only confirmed hits. This directly attacks the measured
123.065 s cache-open failure while preserving target-level reuse.

- Instrument first: cache bytes, bytes actually read, header-open time,
  per-target decode time, peak RSS, and compression ratio.
- Reject unless cold cache open plus decode is below 15 s and end-to-end rebuild
  beats the current 116.592 s by at least 5%.
- Correctness must remain fail-closed for card replacement, case-only rename,
  coarse timestamp collisions, archive changes, and corrupt/truncated frames.

### 3. Make shard overlap phase-aware

The current pipeline overlaps about 19 s, yet copy/hash grows from the earlier
serialized 8 s range to roughly 25 s under contention. Test a bounded schedule
that builds the largest shard first, begins its publication while constructing
small shards, and prevents simultaneous heavy SQLite finalization and exFAT
copy/hash. Keep publication serialized.

- Before/after metric: shard batch wall, build wall, publication wall, queue
  wait, block I/O, and CPU utilization by core.
- Reject unless shard batch improves by at least 5% with identical artifacts.
- Conservative opportunity: 4-10 s from full clean build.

### 4. Sequential generation writer for tmpfs-built shards

The remaining 73.3 MB copy/hash phase is the largest clean-build publication
ceiling. Prototype serializing finalized SQLite and navigation artifacts into a
generation-private sequential exFAT writer that hashes the exact bytes as they
are written, then syncs and renames them. This is not the already-rejected idea
of constructing random-write SQLite databases directly on exFAT.

- Before/after metric: bytes read/written, copy/hash wall, block requests,
  sync/rename wall, and full shard-batch wall.
- Fault tests: interruption during write, file sync, rename, directory sync,
  and manifest publication; the prior generation must remain authoritative.
- Reject unless end-to-end clean build improves by at least 5%.

## Qualification caveats

The final report contains one three-leg whole-card sequence, not a statistical
distribution. It is strong correctness evidence and a matched before/after
observation, but additional runs are required before turning small differences
into broad performance claims. The large retained rebuild delta and the
repeatable 166-167 s device clean-build pair are the most credible results.
