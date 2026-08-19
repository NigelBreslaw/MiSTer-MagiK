# Catalog Build And Rebuild Optimization

Date: 2026-08-19

## Outcome

The retained optimization reuses the live, already-audited Arcade bootstrap
scan in the authoritative full catalog scan. On the real exFAT card, with real
Arcade and SNES content in every sample, median fresh completion fell from
66.935 s to 22.702 s: 44.233 s faster, or 66.1%.

First-visible Arcade remained in the same band: 8.890 s before and 9.263 s
after. The final build completed before the benchmark observed any real Arcade
selection movement in all three samples, so catalog work did not overlap the
post-intro scrolling window. This is not a claim that every frame of the intro
animation was repetition-free; the UI qualification is deliberately scoped to
observed application movement.

The duplicate durable-target walk experiment was rejected and reverted. It
changed rebuild median from 18.927 s to 18.825 s, only 102 ms or 0.5%, and did
not justify the extra resume-path complexity.

## Whole-Card Confirmation

The typed `catalog-full-build-rebuild` scenario then removed only an isolated
output catalog, used every normally configured library source on the card, and
preserved the completed fresh result for a forced same-boot rebuild. It did not
drop the Linux page cache. The installed runtime remained `95542f01d` for both
legs.

| Metric | Result |
|---|---:|
| Fresh Arcade first visible | **9.437 s** |
| Fresh whole-card completion | 272.112 s (4m 32.112s) |
| Forced same-boot rebuild | 169.457 s (2m 49.457s) |
| Published content | 69 systems, 39,791 games |
| Fresh/rebuild count parity | exact |

Evidence: [whole-card summary](../../build/agent-benchmarks/catalog-full-build-rebuild/1787160204/summary.json).

This confirms that the retained Arcade reuse still delivers a usable Arcade
catalog in under ten seconds even when the remaining 68 systems are scanned.
It does not make the complete whole-card catalog a sub-ten-second operation.
The 102.655 s gap between the fresh and immediately following rebuild is a new
optimization signal, but it cannot yet be assigned wholly to page-cache warmth,
durable namespace reuse, or first-publication work because this scenario does
not expose those phase boundaries. The rebuild's 169.457 s is in the same band
as the earlier 172.63-174.63 s warm whole-card attribution runs.

## Workload And Evidence Contract

The typed scenario is `scripts/agent benchmark catalog-build-rebuild`.
Each of three samples:

1. removes only the isolated benchmark catalog and synthetic overlay;
2. catalogs real `/media/fat/_Arcade` and real `/media/fat/games/SNES`;
3. includes one isolated synthetic SNES file so correctness can be mutated;
4. requires exactly Arcade and SNES in the published catalog;
5. adds a second synthetic SNES file and forces a rebuild;
6. requires the SNES count to increase by exactly one;
7. records physical FPGA-owned-vblank evidence only after a real Arcade
   selection change, or records that catalog completion preceded interaction.

The allowlist prevents unrelated systems from turning each development sample
into a whole-card scan. Arcade is never synthetic and is mandatory in every
leg. Real SNES contributes 1,803 games; the overlay changes that count to 1,804
on rebuild. Arcade contributes 968 games.

Evidence:

- Baseline, installed `772315c10`:
  [summary](../../build/agent-benchmarks/catalog-build-rebuild/1787155646/summary.json)
- Duplicate-walk experiment, installed `d54eb4918`:
  [summary](../../build/agent-benchmarks/catalog-build-rebuild/1787156190/summary.json)
- Final retained runtime, installed `95542f01d`:
  [summary](../../build/agent-benchmarks/catalog-build-rebuild/1787157554/summary.json)

## Before And After

| Metric | Baseline | Duplicate-walk experiment | Final Arcade reuse | Final vs baseline |
|---|---:|---:|---:|---:|
| Fresh first visible, median | 8.890 s | not a target | 9.263 s | +0.373 s |
| Fresh complete, median | 66.935 s | 67.271 s | 22.702 s | **-44.233 s (-66.1%)** |
| Rebuild complete, median | 18.927 s | 18.825 s | 18.848 s | -0.079 s (-0.4%) |
| Fresh catalog | 968 Arcade + 1,803 SNES | same | same | exact |
| Rebuild proof | SNES +1 in 3/3 | SNES +1 in 3/3 | SNES +1 in 3/3 | exact |
| Scroll overlap | active, 0 dropped frames in 3/3 | one unrelated fresh sample failed at 1 repeated frame | none; build finished first in 3/3 | desired first-boot ordering |

The duplicate-walk experiment correctly avoided a second assembly traversal
for fingerprint-matched durable targets, but end-to-end rebuild time did not
move materially. This shows the remaining 18.8 s rebuild is dominated by the
validation traversal and later phases, not the redundant consumer traversal.

The Arcade reuse result is large because the first-visible live scan used to be
discarded. The full builder then walked and classified the same Arcade target
again while sustained Arcade interaction competed with catalog work. The
retained path consumes the live RAM artifact once, excludes only the identical
Arcade target from the second scan, and still performs normal authoritative
projection and publication. A retained mini-nav index is never treated as scan
authority, and restart recovery conservatively rescans when no live artifact
exists.

## Logical Commits And Checklists

### Bounded benchmark harness

Commits: `8c8eafd99`, `fbe3af4b8`, `cc986d318`, `1c0fd522a`,
`772315c10`, `eb5a85102`, `1a9ec90a1`, `9d583cad0`, `8e04e7936`.

- [x] Add a typed three-sample catalog build/rebuild scenario.
- [x] Isolate every generated catalog, lock, snapshot, and synthetic file.
- [x] Always scan real Arcade and real SNES.
- [x] Enforce the target allowlist for static, runtime, and facts-only targets.
- [x] Prove the one-file SNES delta after every rebuild.
- [x] Drive input through typed launcher automation.
- [x] Gate scroll evidence on observed selection movement.
- [x] Preserve installed revision and physical refresh evidence.

### Duplicate durable-target traversal

Experiment: `d54eb4918`; rejection: `95542f01d`.

- [x] Decode reusable target output before deciding to skip traversal.
- [x] Emit stable target start/complete boundaries without payload events.
- [x] Fall back to a full target walk for corrupt saved output.
- [x] Add a focused boundary-only regression test.
- [x] Deliver and run the identical three-sample before/after benchmark.
- [x] Reject and revert after only a 0.5% rebuild change.

### Reuse live Arcade bootstrap scan

Commit: `f5f9ea063`.

- [x] Retain the live Arcade RAM scan after first-visible preparation.
- [x] Feed it into foreground and CPU0-background full-scan routes.
- [x] Exclude only the matching Arcade filesystem target.
- [x] Preserve durable-resume fallback and authoritative publication rules.
- [x] Reuse existing merge-equivalence coverage.
- [x] Deliver the production-feature ARM binary and pass smoke validation.
- [x] Pass the final three-sample real-card benchmark.

### Whole-card confirmation harness

Commits: `907460f28`, `c5ff459b6`.

- [x] Add a typed one-sample whole-card fresh-plus-rebuild scenario.
- [x] Redirect every generated catalog artifact to isolated exFAT paths.
- [x] Use the normal configured roots without a target allowlist.
- [x] Preserve the fresh catalog for a forced newer rebuild generation.
- [x] Require exact system and game-count parity between both legs.
- [x] Keep UI scrolling qualification in the bounded scenario rather than
  racing it into the whole-card timing measurement.
- [x] Verify installed manifest and boot identity remain unchanged.

## Next Optimization Ideas From The New Data

1. **Make the pre-input deadline explicit.** The retained build now completes
   in 22.3-22.9 s and already beats observed selection movement, but it is close
   enough to the nominal 20-second intro to merit margin. Run a capacity-one
   publication queue with a second shard-construction worker only while the
   intro has measured frame slack; immediately return all catalog work to CPU0
   when application movement becomes possible. This uses the second Cortex-A9
   without allowing two exFAT publishers or risking scroll contention.

2. **Hash while writing and rename in the final directory.** The broader
   attribution run measured 8.18-8.42 s copying and hashing 73,290,597 bytes
   during publication. Stream the digest while producing each temporary shard,
   sync it, rename in place, and retain the manifest barrier. Start with one
   large shard and require fault/recovery parity. See the
   [performance attribution report](../toolchain-bench/performance-attribution-20260819.md).

3. **Replace validation walks with trustworthy namespace change receipts.**
   Removing the second durable-target walk did not improve the 18.8 s bounded
   rebuild, so the first fingerprint walk is the target. Persist per-target
   namespace snapshots, accept explicit dirty receipts from updater/install
   flows, bind them to the exFAT volume identity, and fall back to a real walk
   after offline or unrecognized changes. Add/remove/rename and power-loss
   cases must produce byte-identical catalog hashes before enabling this beyond
   one target. The whole-card attribution data gives this a 54-55 s cold-cache
   ceiling.

4. **Separate scan completion from publication readiness in telemetry.** Add a
   benchmark field for `scan-complete`, `projection-complete`, and
   `interaction-first-movement`. That will reveal whether the remaining
   2-3-second deadline margin is best won in scan, shard construction, exFAT
   publication, or launcher adoption without using a full-card run.

5. **Split first-publication cost from warm rebuild cost.** The whole-card pair
   exposed a 102.655 s fresh/rebuild gap on one boot. Extend the typed scenario
   with existing catalog phase markers and namespace-snapshot hit counts, then
   repeat alternating isolated-output and preserved-output legs. This will show
   whether the next large win belongs in cold exFAT discovery, initial shard
   construction, or snapshot seeding instead of inferring it from one total.

Run the instrumentation in 5 before another whole-card implementation. The
optimization priority remains 2, then a bounded version of 1, then 3. Idea 2
has the strongest deterministic byte-level evidence. Idea 1 can guarantee
first-boot ordering but must remain animation-slack-aware. Idea 3 has the
largest rebuild ceiling and the highest correctness risk.
