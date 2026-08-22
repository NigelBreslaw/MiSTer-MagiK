# Catalog namespace streaming screen attribution

Date: 2026-08-22

The first exact-device screen compared both namespace backends from the same
Dev binary and production corpus at MagiK revision
`4494da95c84358f20c0b70c8d0e4049d400dbfa3`.

## Artifacts

- Whole-target fd-relative control:
  `build/agent-benchmarks/storage-attribution/1787367580/summary.json`
- Per-entry fd-relative streaming candidate:
  `build/agent-benchmarks/storage-attribution/1787367854/summary.json`

## Results

| Measure | Control | Streaming | Delta |
|---|---:|---:|---:|
| Whole workload | 143.830s | 632.010s | +488.180s (+339.4%) |
| Catalog scan | 74.847s | 561.143s | +486.296s (+649.7%) |
| Namespace producer | 55.842s | 553.964s | +498.122s (+892.0%) |
| Channel wait | 10.792s | 0.230s | -10.562s (-97.9%) |
| Consumer wait | 11.070s | 372.548s | +361.478s (+3,265.5%) |
| Consumer active | 61.112s | 186.728s | +125.616s (+205.5%) |
| Walker read bytes | 6,218,536 | 581,912,744 | +575,694,208 (+9,257.7%) |
| Device read bytes | 212,320,256 | 252,329,472 | +40,009,216 (+18.8%) |
| Peak buffered entries | 5,963 | 1 | -5,962 (-100.0%) |
| Buffer allocations | 11,649 | 503 | -11,146 (-95.7%) |
| HWM | 115,308 KiB | 115,220 KiB | -88 KiB (-0.1%) |

Both arms produced 69 systems and 40,013 games with identical identity,
ordering, launch, search, and artifact-set hashes. Neither arm fell back or
restarted.

## Attribution and bounded recovery

The failed phase is fd-relative namespace production on exFAT. The original
streaming DFS seeks the parent directory to `linux_dirent64.d_off` before
descending, discards the unread records already present in the shared 128 KiB
buffer, and requests them again after returning. That reduced handoff wait but
amplified walker input by more than 93 times and starved the consumer.

The one bounded recovery will retain each fetched directory block as pending
entry work, traverse it depth-first without seeking or rereading, and keep an
explicit pending-entry/path-byte ceiling. Any ceiling or syscall failure must
still emit `TargetRestart` before WalkDir fallback. The screen is far below
every performance gate, so no confirmation runs are justified for the failed
implementation.
