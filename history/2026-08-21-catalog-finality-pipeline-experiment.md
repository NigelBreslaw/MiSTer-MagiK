# Catalog finality pipeline experiment — 2026-08-21

Revision tested: `3e7a2e53673b528bcdd95b6352e3c81a7a1adbee`

This experiment tested whether conservative contributor finality exposes enough
whole-card scan time to justify building system shards before the scan ends. It
temporarily drained wildcard targets first, then retained exact-system target
order. It did not start early shard writes.

## Result

Rejected before implementation of early writes. The proven overlap ceiling was
below the required 10 seconds.

- Wildcards drained and C64/PC-88 closed at 29.368 s.
- The scan execution pipeline ended at 34.582 s.
- Maximum possible expensive-shard overlap was therefore 5.214 s.
- The early Arcade shard took only 0.778 s to build and publish.
- GBA, GBC, MegaDrive, and NES closed with less than 1.5 s of scan remaining.

Building the early pipeline could not meet the retained experiment gate even
with perfect scheduling. Revision `05321adb1` therefore restores the qualified
production target order while retaining the exact-identity and contributor-
closure instrumentation.

## Whole-card evidence

`scripts/agent benchmark catalog-full-build-rebuild` passed:

| Leg | Complete | First visible | Peak HWM |
|---|---:|---:|---:|
| First observed clean | 186.136 s | 9.896 s | 139,032 KiB |
| Warm clean | 196.427 s | 15.890 s | 143,632 KiB |
| Forced rebuild | 51.778 s | 4.134 s | 62,544 KiB |

All legs retained 40,059 games, exact canonical/order/launch/search identities,
valid artifact SHA sets, and complete phase evidence. Peak HWM remained below
the 144,328 KiB ceiling.

The fresh result is 11.419 s faster than the immediately preceding qualified
closure run, but the warm result is 51.886 s slower and the earlier recorded
fresh baseline is faster still. The single fixed-order runs do not establish a
causal whole-wall improvement, so no performance claim is retained.

Evidence is in
`build/agent-benchmarks/catalog-full-build-rebuild/1787306191/summary.json` and
the adjacent per-leg launcher logs.

## Follow-up profiling and retained cold-build improvement

Wall timing alone did not support the wildcard-order result. A matched PMU
screen of refined runtime contributor scheduling regressed fresh creation from
100.129 s to 104.927 s while increasing cycles by 3.34% and instructions by
5.53%. PC-88 traversal alone varied from 4.151 s to 14.671 s. Revisions
`413638d80` and `c0667c6f1` retain that failed hypothesis and its revert.

The profiler instead identified a fixed post-publication gap. Each fresh shard
was already reopened and fully validated by the writer, copied or renamed
while hashing, and synchronized as a batch. Recovery checkpointing then opened
and fully validated all 69 published shards again before committing 69 journal
rows in 69 transactions. Resume independently validates every recorded shard
before reuse, and readers remain gated by the manifest-last publication.

The retained implementation therefore:

- records a durable shard batch in one SQLite transaction (`0975dbdd0`);
- omits only the immediate duplicate validation before that journal write
  (`4fd0e82fb`);
- retains writer validation, artifact hashing, the artifact durability barrier,
  resume validation, and manifest-last authority;
- profiles allocator trimming explicitly (`ac41eeef7`, `e79d38fa8`) and leaves
  its policy unchanged after measuring only 0.284 s across the 69 fresh shards.

### PMU campaign

The behavior-matched instrumented baseline is
`build/agent-benchmarks/pmu-profile/1787309786/summary.json`. The final candidate
screen and confirmations are `1787310873`, `1787312414`, and `1787312726`.

| Metric | Instrumented baseline | Candidate samples | Candidate median | Delta |
|---|---:|---:|---:|---:|
| Fresh operation | 102.715 s | 90.166 / 97.392 / 94.948 s | 94.948 s | -7.767 s (-7.56%) |
| Fresh shard batch | 33.387 s | 25.738 / 25.805 / 25.887 s | 25.805 s | -7.582 s (-22.71%) |
| Changed shard batch | 1.293 s | 1.106 / 1.298 / 1.104 s | 1.106 s | -0.187 s (-14.46%) |
| Rebuild-all shard batch | 27.287 s | 28.217 / 27.371 / 28.473 s | 28.217 s | +0.930 s (+3.41%) |

Fresh peak HWM remained between 113,496 and 114,776 KiB. Every suite passed
the exact +1 SNES mutation, preserved rebuild-all counts, produced complete PMU
profiles, and cleaned its isolated root. Whole changed-rebuild and rebuild-all
wall times remained dominated by scan variance; the changed shard phase itself
improved, while the rebuild-all shard phase showed a small secondary regression.

### Unprofiled whole-card evidence

`build/agent-benchmarks/catalog-full-build-rebuild/1787311532/summary.json`
passed at `4fd0e82fb`:

| Leg | Complete | First visible | Builder persisted |
|---|---:|---:|---:|
| First observed clean | 126.293 s | 9.941 s | 107.418 s |
| Warm clean | 132.990 s | 13.688 s | — |
| Forced rebuild | 71.234 s | 2.706 s | — |

Against the qualified `1787300445` cold builder time of 115.380 s, first
creation improved by 7.962 s (6.90%). Its fresh shard batch improved from
39.861 s to 31.749 s, a causal 8.111 s (20.35%) reduction. Peak HWM was
142,108 KiB, below the 144,328 KiB gate. All three legs retained 69 systems,
40,059 games, exact row/order/launch/search identity and artifact sets. X68000
retained 273 source games, 269 visible families, and four intended collapses.

Two additional full-sequence attempts failed after launcher restoration because
the bounded telemetry collector returned no samples. Two `catalog-lifecycle`
attempts likewise completed restoration but captured no authoritative startup
intro cadence. They are neither catalog passes nor failures and are not counted
as confirmation evidence.

## Interruption exposure

Completed scan targets are durable in batches of at most 16 targets or 2 MiB;
the recorded whole-card run committed all 160 targets in 14 batches. Launching a
game during scanning can therefore lose only the uncommitted target tail.

System contributor closure is still telemetry rather than a reconciliation
trigger. After scan completion, roughly 20 s of global preparation remains, and
fresh shard recovery records are still committed near the end of the shard
pipeline. The retained optimization shortens that exposure by about eight
seconds but does not make closed systems independently durable. Bounded
mid-pipeline shard checkpoints were therefore measured next, with their extra
exFAT synchronization barriers treated as an explicit cold-creation tradeoff.

## Bounded restart-recovery checkpoints

Revision `82d498ad6` accepts a small cold-build cost in exchange for materially
better recovery when MagiK exits before first catalog publication. The fresh
pipeline now synchronizes and journals each eight-shard batch, then flushes the
final tail on handled completion. Resume keeps the existing full validation of
every saved shard, and the manifest remains the sole reader authority.

`build/agent-benchmarks/catalog-full-build-rebuild/1787313788/summary.json`
passed on the exact installed revision:

| Cold product metric | Single-checkpoint candidate | Eight-shard checkpoints | Delta |
|---|---:|---:|---:|
| Builder persisted | 107.418 s | 107.902 s | +0.485 s (+0.45%) |
| Fresh shard batch | 31.749 s | 32.383 s | +0.634 s (+2.00%) |
| First visible | 9.941 s | 8.876 s | -1.065 s |
| Peak HWM | 142,108 KiB | 133,732 KiB | -8,376 KiB |

Against the qualified pre-optimization baseline, the retained cold builder
improvement remains 7.478 s (6.48%), and the fresh shard phase remains 7.477 s
(18.76%) faster. The run recorded durable totals of 8, 16, 24, 32, 40, 48,
56, 64, and 69 shards. A sudden exit can therefore discard only the current
eight-shard durability window plus any publication already in the two-slot
pipeline, rather than all 69 completed shards.

All legs retained 69 systems and 40,059 games with identical exact identities
and valid artifact sets. X68000 retained 273 source games, 269 visible families,
and four intended collapses. Warm peak HWM was 140,608 KiB, below the 144,328
KiB gate. Outer harness completion varied to 202.526 s cold, 167.406 s warm,
and 59.369 s rebuild; the stable product builder and shard-phase clocks isolate
the checkpoint cost from that scan and post-build variance.
