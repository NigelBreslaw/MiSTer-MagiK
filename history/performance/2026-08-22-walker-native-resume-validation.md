# Walker-native resume validation — 2026-08-22

## Decision

Retained for production. The candidate narrowly missed the original 5% median
whole-recovery gate but reached 4.57%, improved two of three matched pairs, and
removed the validation event/consumer path without changing any catalog or
artifact identity. This is accepted under the operator direction to retain
useful near-gate improvements and move on.

The evidence does **not** claim a 5% validation-phase improvement. exFAT
namespace traversal remains dominant and the validation-phase median improved
only 0.39%.

## Authority

- Baseline runtime revision: `1b0569140999ec070b07348f1238e7d9f07b784f`
- Baseline artifact:
  `build/agent-benchmarks/catalog-resume-validation/1787410850`
- Experiment runtime revision: `61ec1fb2aec93c47e9d3ea8a8abed81085943219`
- Paired artifact:
  `build/agent-benchmarks/catalog-resume-validation/1787411656`
- Retained production revision: `271f532fd76843a6f0c5079852e4f99d9ea75299`
- Production-only artifact:
  `build/agent-benchmarks/catalog-resume-validation/1787412397`
- Device boot ID remained
  `bb2f71ac-ed0d-43bd-be6b-e1ddda37507b`.
- Interruption was an ordinary Dev launcher restart after a synced target
  checkpoint. Direct-reset fault injection and forced background catalog work
  were both disabled.

## Results

| Metric | Event baseline | Walker-native | Delta |
| --- | ---: | ---: | ---: |
| Recovery completion median | 42.778s | 40.824s | -1.954s (-4.57%) |
| Validation median | 3.895s | 3.880s | -15ms (-0.39%) |
| Validation consumer median | 82.942ms | 0.076ms | -82.866ms (-99.91%) |
| Channel send median | 10.628ms | 0 | -10.628ms (-100%) |
| Validation events | 4,881 | 0 | -4,881 (-100%) |
| Peak HWM median | 102,568KiB | 102,600KiB | +32KiB (+0.03%) |

Paired recovery completion:

- Pair 1: 42.866s → 40.824s (-4.76%).
- Pair 2: 42.778s → 40.438s (-5.47%).
- Pair 3: 40.808s → 41.649s (+2.06%).

All six arms reused four committed targets, invalidated none, produced one
identical set of identity/ordering/launch/search hashes, validated every
artifact set, retained the same boot, and left both production registries
byte-identical.

After selector removal, three production-only controls completed in 40.214s,
41.513s, and 42.456s (41.513s median). Validation completed in 4.417s, 3.740s,
and 3.789s (3.789s median), emitted zero validation events, reused all four
targets, and invalidated none. The production report identified
`walker-native` in all three samples and again preserved exact identities,
artifacts, boot ID, and production registries.

## Attribution and remaining opportunity

The candidate removed the intended per-entry handoff but did not remove the
namespace backend's deterministic whole-target capture. Both arms therefore
reported 39,414 captured entries, while the walker owned approximately
3.7–4.25s. A materially larger recovery gain requires a separately qualified
streaming namespace backend; this item does not reopen that broader catalog
experiment.

The exact-device arm covered current production MRA, SNES, C64, runtime facts,
and compact-frame reuse. It did not run the complete synthetic mutation matrix;
the shared encoder parity test and exact artifact controls are the retained
correctness authority for this bounded promotion.
