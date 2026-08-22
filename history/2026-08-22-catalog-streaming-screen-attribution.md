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

## Bounded recovery result

The no-reread task-stack recovery was screened at revision
`7d138cfa32cdb6ad725697ea8dc27c74551c5b55`:

- Control: `build/agent-benchmarks/storage-attribution/1787369076/summary.json`
- Recovery: `build/agent-benchmarks/storage-attribution/1787369339/summary.json`

| Measure | Fresh control | Recovery | Delta |
|---|---:|---:|---:|
| Whole workload | 142.110s | 149.840s | +7.730s (+5.4%) |
| Catalog scan | 72.731s | 77.354s | +4.622s (+6.4%) |
| Namespace producer | 56.942s | 64.466s | +7.524s (+13.2%) |
| Channel wait | 8.459s | 5.474s | -2.985s (-35.3%) |
| Consumer wait | 12.844s | 14.906s | +2.062s (+16.1%) |
| Consumer active | 57.529s | 60.044s | +2.515s (+4.4%) |
| Walker read bytes | 6,218,536 | 6,218,536 | 0 (0.0%) |
| Walker read calls | 20,548 | 20,548 | 0 (0.0%) |
| Device read bytes | 218,915,840 | 229,176,320 | +10,260,480 (+4.7%) |
| Buffer allocations | 11,649 | 1,631 | -10,018 (-86.0%) |
| Peak buffered bytes | 978,524 | 292,877 | -685,647 (-70.1%) |
| HWM | 115,300 KiB | 115,364 KiB | +64 KiB (+0.1%) |

The recovery again preserved all behavior and artifact hashes with zero
fallbacks or restarts. It removed the diagnosed read amplification, but the
task-stack/path-copy overhead increased producer and consumer work and missed
both the 5% scan-improvement gate and 2% whole-operation gate. It achieved less
than 80% of either gate, so the roadmap does not permit a second recovery. The
streaming selector and implementation are explicitly reverted; restartable
target handoff remains as a neutral correctness prerequisite.
