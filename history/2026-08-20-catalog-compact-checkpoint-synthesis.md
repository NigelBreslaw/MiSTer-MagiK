# Catalog optimization review synthesis

Frozen evidence commit: `8433aa13c`

Qualified code revision: `9fe598fdcddccf7c2ef9525d56b994e46427176d`

Starting revision: `0fc7b19fcefe7b3d6f28fa5c43ed06e7e81c15d4`

## Campaign outcome

Compact checkpoint frames are the only retained optimization. The strongest
causal evidence is the same-boot, three-sample bounded comparison: checkpoint
write time fell 95.1%, fresh completion fell 10.9%, internal builder completion
fell 13.4%, rebuild stayed effectively flat, and internal Arcade readiness
regressed only 0.93%. The exact catalog fingerprint was unchanged and RSS grew
by 3,192 KiB, within the absolute limit.

The final bounded times were 49.598 s fresh, 15.450 s rebuild, and 8.679 s
first-visible. The final whole-card observations were 223.667 s first clean,
156.219 s warm clean, and 67.930 s rebuild for 69 systems and 39,791 games,
with exact fingerprint parity. The corresponding frozen starting observations
were 266.703 s, 240.747 s, and 77.599 s. These whole-card comparisons are useful
qualification evidence but are not causal estimates: there is one sample per
leg, fixed order, unchanged cache policy, a different boot, and variable host
polling gaps.

Exact target replay, its bincode correction, SQL multi-row FTS batching, and
direct on-media shard construction failed their gates. They were explicitly
reverted or kept out of production selection.

The three independent reviewers verified all 14 hashes in the frozen manifest.
During synthesis, one omitted control artifact used for the on-media phase
comparison was restored to the chain of custody as supplemental review
evidence:

- `build/agent-benchmarks/catalog-attribution-control/1787232651/summary.json`
- SHA-256 `57885405b1dd4b4bdc400bd070abc77f0c80ca8554a214053ed3beea2678c08a`
- Revision `5778fd447a736a4c9e7475c642e81eac2e9e1c0a`

This supplement does not mutate the frozen evidence commit and does not alter
the rejection: direct on-media construction lost by 7.978 s on fresh completion
despite its faster publication phase.

## Ranked next work

Scores are 1-5. Higher evidence and impact are better; higher risk and cost are
worse. Projected gains are conservative hypotheses, not measured wins.

| Tier | Mechanism | Evidence | Impact | Implementation risk | UI/correctness risk | Experiment cost | Conservative operation gain |
|---|---|---:|---:|---:|---:|---:|---|
| 1 | Stream/partition large checkpoint frames and reduce metadata commits | 5 | 3 | 3 | 3 | 2 | 2-5 s clean whole-card |
| 1 | Reduce tmpfs-to-exFAT publication amplification | 4 | 4 | 3 | 4 | 3 | 3-8 s clean whole-card |
| 2 | Instrument and restructure scan producer/classifier batching | 4 | 5 | 4 | 5 | 4 | 5-12 s clean/rebuild |
| 2 | Instrument a system-finality frontier for earlier shard work | 3 | 5 | 5 | 5 | 4 | 4-10 s clean if a 10 s frontier exists |
| 2 | Instrument Arcade MRA fusion versus minimal projection | 4 | 2 | 3 | 4 | 3 | 0.5-2.5 s first-visible |
| 2 | Prototype a true FTS bulk-build architecture | 3 | 2 | 4 | 4 | 3 | 1-2 s bounded; 2-6 s whole-card |

### Tier 1: experiment now

1. Further checkpoint streaming is supported by the retained mechanism and a
   remaining measured 5.92-11.56 s whole-card checkpoint phase. Preserve frame
   hashes, interruption recovery, and strict HWM bounds. Require at least 15%
   checkpoint and 5% internal-completion improvement.
2. Publication work should retain tmpfs SQLite construction and test
   preallocation, kernel-assisted copy, or phase-aware copy scheduling. Require
   at least 20% publication and 5% whole-operation improvement with exact hashes
   and manifest-last recovery. Direct on-media SQLite is already falsified.

### Tier 2: instrument first

1. All reviewers found scan/classification headroom, but they proposed different
   mechanisms: target batches, post-reveal CPU1 help, or bounded two-lane work.
   First separate namespace, archive, classification, queue, affinity, and HWM
   costs. No concurrency candidate should proceed without physical UI evidence
   if it uses CPU1.
2. Record the point at which each system is immutable before attempting to
   overlap its shard construction with remaining scan work. Proceed only if the
   trace predicts at least 10 s of safe overlap.
3. Arcade's first-visible ceiling is split between MRA I/O and a measured
   3.036 s projection phase. Instrument both before selecting either fused
   parsing or a minimal first-visible projection.
4. A true FTS bulk-build may remain worthwhile, but it must be architecturally
   different from the rejected SQL statement batching and preserve explicit
   row IDs, ranks, tokenizer behavior, autocomplete ordering, optimize, and
   integrity checks.

## Rejected mechanisms

- Do not retry exact target replay with the current validate-then-decode design.
- Do not retry a codec-only correction to that replay design.
- Do not retry multi-row prepared FTS inserts.
- Do not select direct on-media SQLite/shard construction for production auto.
- Do not introduce a metadata-only unchanged probe.
- Do not add shard workers blindly; the existing pipeline already overlaps
  build and publication and shows queue pressure.
- Do not weaken hashing, synchronization, integrity checks, or manifest-last
  publication to manufacture a timing win.

## Dissent and limitations

- The systems reviewer sees meaningful CPU1 capacity after Arcade reveal, but
  the measurement reviewer requires instrumentation first and the frozen runs
  contain no applicable UI qualification. This remains instrument-first.
- The data-path reviewer prefers target-sized batches; the systems reviewer
  prefers a cooperative CPU1 helper. Both address the same scan/classification
  ceiling, so neither is promoted until attribution distinguishes the mechanism.
- The data-path reviewer proposes a minimal Arcade projection while the systems
  reviewer proposes fused MRA discovery. The evidence identifies both ceilings
  but does not yet choose between them.
- Whole-card results have one fixed-order sample per leg. Current RSS is not a
  substitute for HWM, and host scenario wall time contains polling gaps. Future
  retention decisions must use matched internal timings, explicit peak memory,
  and exact output checks.
