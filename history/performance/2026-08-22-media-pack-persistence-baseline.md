# Media-pack persistence baseline — 2026-08-22

## Scope

- Current production path: network stream plus SHA-256 into tmpfs, verification,
  then a buffered copy to a hidden exFAT sibling followed by sync, rename, and
  parent sync.
- Production format: raw `.mmlz4b`; decode time is zero.
- Exact Dev runtime: MagiK `60dc99801`, Main `639d3694e`.
- Catalog refresh remained off; the benchmark did not force background work.
- Artifact: `build/agent-benchmarks/media-pack-persistence/1787408509`.

The typed benchmark selected the small, median, and largest packs at the
production image size, primed the remote cache, and ran three staged controls.
All nine rows were Cloudflare cache hits, used hidden benchmark paths, preserved
the expected pack hashes, and cleaned their temporary pack and state files.

## Three-run medians

| Pack | Bytes | Network + tmpfs | Verify | tmpfs → exFAT save | Total |
|---|---:|---:|---:|---:|---:|
| NeoGeo | 4,973,975 | 0.359 s | 0.119 s | 0.347 s | 0.882 s |
| Arcade | 24,326,278 | 1.245 s | 0.604 s | 2.052 s | 3.906 s |
| Amiga | 47,654,942 | 2.370 s | 1.143 s | 3.859 s | 7.381 s |

Process HWM moved from 5,896 KiB to 6,668 KiB. Each row wrote one pack-sized
copy to tmpfs and one to exFAT. The isolated persistence arm records index
identity but does not download it; exFAT writer concurrency is one.

## Reopening rationale

Direct sibling-exFAT streaming was retained in June after improving total flow
by 19.2% to 22.6%, then replaced by tmpfs staging specifically to prevent
exFAT writes from throttling media downloads during automatic background
catalog scanning. MagiK no longer performs that scan during ordinary boots;
catalog work beside the UI now occurs only for the genuine first boot with no
catalog or after an explicit user rebuild. Forced background catalog work is
therefore not a qualification gate for this item. The large current save phase
justifies a fresh production-off direct-stream comparison.

## Direct-stream comparison and retention

Experiment revision `b1ea232a2` ran three alternating staged/direct pairs in
`build/agent-benchmarks/media-pack-persistence/1787408792`. Every arm selected
the same three pack identities and byte counts, matched the production hashes,
reported `bench-ok`, and removed its hidden pack/state artifacts.

| Pack | Staged total | Direct total | Total delta | Staged save | Direct finalize | Save delta |
|---|---:|---:|---:|---:|---:|---:|
| NeoGeo | 0.899 s | 0.790 s | -12.1% | 0.400 s | 0.033 s | -91.8% |
| Arcade | 3.755 s | 3.285 s | -12.5% | 1.923 s | 0.062 s | -96.8% |
| Amiga | 7.038 s | 6.100 s | -13.3% | 3.607 s | 0.118 s | -96.7% |

Candidate HWM was 6,568 KiB versus 6,620 KiB for staged controls, and tmpfs
pack bytes fell from one full pack per row to zero. Direct download/write
throughput was 53–64 Mbit/s versus 110–160 Mbit/s for tmpfs staging because the
same unavoidable exFAT write moved into that phase. That misses the isolated
network-throughput sub-gate, but every representative end-to-end flow finishes
12.1–13.3% sooner and the separate post-download wait is nearly eliminated.

The original reason for reinstating tmpfs was interference from automatic
background catalog scans, which no longer occur. With catalog refresh off and
no forced-background requirement, the throughput movement is not a product
regression: it changes when the user sees the exFAT work, reduces the time to a
usable pack, and lowers memory. Production therefore retains direct streaming,
with transactional sync/rename ordering and a bounded tmpfs recovery path only
for destination create/write/flush failures. Network, size, and hash failures
are never replayed automatically.

## Retained production control

The production-only authority passed after Dev delivery of `930d62582`:

- Artifact: `build/agent-benchmarks/media-pack-persistence/1787409664`
- NeoGeo median: 0.781 s total, 0.019 s finalize.
- Arcade median: 2.932 s total, 0.062 s finalize.
- Amiga median: 6.229 s total, 0.117 s finalize.
- Process HWM: 6,576 KiB.
- Nine of nine rows were `bench-ok` with the expected pack byte counts and
  SHA-256 identities.

The typed authority now exposes only the retained direct-stream production
path; the staged/direct selector is absent.
