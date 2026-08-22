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
