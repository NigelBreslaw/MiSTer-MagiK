# Independent Cortex-A9/exFAT review v2

Frozen evidence commit: `9f7833056`

Qualified runtime: `99f6b36399563ed8b5aef883f93a7ca8921e0402`

Role: Cortex-A9/exFAT systems reviewer. This review was performed read-only and
without access to the other reviews.

## Verdict

All 14 manifest artifacts match their SHA-256 values. The strongest supported
retained mechanisms are compact sequential checkpoint frames, the full-audit
post-scan unchanged exit, and tmpfs shard construction with serialized,
manifest-last exFAT publication.

The work-mode/dual-core mechanism is only partially validated. Frozen logs
prove mode transitions, but not the actual post-checkpoint all-online affinity,
and no frozen qualification leg exercises `Paused` or parked background
threads. “Scroll parking validated” and “dual-core execution proven” would
overstate this evidence.

## Measured system facts

- Bounded real-root fresh median: 91.260 s; changed rebuild: 30.248 s.
- Whole-card clean/warm/rebuild: 219.913/165.110/66.071 s.
- Whole-card scan remains 45.75-62.13 s.
- Clean copy/hash is 18.85-19.65 s for 73,313,125 bytes; total publication is
  19.86-21.07 s and overlaps build by 13.23-14.85 s.
- Clean checkpoint input is about 31.47 MB compressed to 3.292 MB. Qualified
  write time is 5.50-6.83 s with zero errors.
- HWM is 133,768/144,328 KiB on clean legs, 61,848 KiB on unchanged rebuild,
  89,296-89,896 KiB bounded fresh, and 83,536-85,636 KiB bounded rebuild.
- Older isolated checkpoint evidence reduces checkpoint write median from
  7.418 s to 0.365 s and bounded fresh from 60.868 s to 54.232 s. That older
  A/B is the clean per-mechanism attribution; v2 revalidates the combined path.

## Retained mechanism validation

- Media discovery is deferred while the catalog gate is active, then released
  when idle. This delays preview freshness but not catalog identity.
- MRA parsing stops at the first ROM payload and Arcade bootstrap uses four
  CPU0 readers. The frozen fresh leg reports 3,004/3,004 successes in 0.536 s.
- The unchanged exit completes coverage audit, exact stored catalog-state
  comparison, and current production-binding validation before returning.
- Compact frames validate extents, chunk bounds, raw lengths, SHA-256, UTF-8,
  and sync frame data before committing SQLite references. Successful-state
  publication uses same-parent rename and parent synchronization.
- Work-mode source policy is coherent: pre-visible `DualCoreBurst`, active
  interaction `Paused`, visible animation CPU0, and stationary idle burst after
  150 ms. Background checkpoints park or change affinity. Qualification logs
  show mode changes but `park_count=0` and no `Paused` sample.
- Search pipelining is bounded to a 256-row producer and capacity-one channel,
  retains ordered row IDs and lexical autocomplete order, and checkpoints both
  lanes. Performance is not isolated and warm HWM is the caution.
- Preallocation remains only a screened route; `copy_file_range` remains
  unusable on this exFAT card. Parallel classification and global autocomplete
  are correctly reverted.
- Publication validates and synchronizes artifacts before manifest-last
  authority; the prior generation remains the recovery authority.

## Dissent

- `interactive_samples_qualified=true` is a harness predicate, not proof of
  authoritative physical cadence. No frozen run isolates repeats at work-mode
  transitions.
- No log proves post-checkpoint CPU0+CPU1 affinity or parking latency.
- Search pipelining and aggressive bursting lack isolated retained A/Bs.
- Warm HWM is 10.6 MiB above first-observed clean.
- The MRA ceiling is about 0.536 s and is not a high-value next target.

## Top ideas: instrument first

### 1. Adaptive scan/classifier batching with measured CPU1 participation

- Evidence: scan is 45.75-62.13 s. First clean records material producer and
  consumer/wait headroom, but affinity is silent.
- Conservative impact: fresh 5-12 s; rebuild 4-10 s; Arcade readiness 0-0.8 s.
- Test real Arcade/SNES/C64 plus PC88 or another archive/runtime target.
- Measure producer, consumer, queue high-water, per-core CPU, applied affinity,
  parking latency, HWM, first-visible, and physical repeats during scripted
  scroll.
- Falsify below 10% scan gain, below 5% whole gain, above 150 MiB HWM, on any
  catalog mismatch, above 16.7 ms park latency, or on any interaction repeat.
- Confidence: medium.

### 2. Phase-aware checkpoint/publication scheduling

Avoid checkpoint sync/SQLite finalization competing with exFAT copy/hash while
preserving a one-entry shard handoff and manifest-last authority.

- Conservative impact: fresh 3-8 s; rebuild and Arcade readiness approximately
  zero.
- Test real Arcade/SNES/C64 with current tmpfs staging.
- Measure full fresh wall, shard build/publication/copy-hash, checkpoint
  sync/commit, block I/O, queue wait, overlap, and HWM.
- Falsify below 20% publication gain, below 5% whole gain, on any hash/recovery
  failure, or above 3% rebuild/Arcade regression.
- Confidence: medium-high.

### 3. Minimal immutable Arcade first-visible projection

Build only the exact launch/navigation subset required for the selected Arcade
tile, then complete full search and cold metadata later under the same
generation contract.

- Conservative impact: Arcade readiness 1-2.5 s; fresh completion 0-1 s;
  rebuild approximately zero.
- Test real Arcade, retaining SNES/C64 to prove final-catalog identity.
- Measure Arcade scan-to-ready, bytes/rows, checkpoint cost, final shard time,
  canonical final results, HWM, and first interaction.
- Falsify below 1 s readiness gain, above 3% full-fresh regression, on any
  final launch/search/navigation mismatch, or if an interrupted partial can
  become authoritative.
- Confidence: medium-low pending row/byte attribution.
