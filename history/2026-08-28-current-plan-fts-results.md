# Current-plan FTS experiment results

Date: 2026-08-28

These measurements exercise the current-plan runtime at commit `75b962c19` on
the real MiSTer using the matched `_Arcade`, `games/SNES`, and `games/C64`
roots. Media activity was disabled. Each benchmark contains three samples
without an intervening reboot; the optimized and unoptimized cohorts were
separated by attended reboots. The first fresh sample is retained as a cold
observation, while medians describe the repeated samples and must not be
treated as a reboot-cold distribution.

## Cohorts

| Cohort | Evidence | Boot | FTS optimize | FTS integrity | Fresh complete (sample 1) | Fresh complete (median) | Runtime complete (median) |
|---|---|---|---|---|---:|---:|---:|
| Bounded, optimized | `build/agent-benchmarks/catalog-attribution-control/1787919247/summary.json` | `022284b9` | on | bounded | 104.329 s | 42.654 s | 38.665 s |
| Bounded, disabled optimize | `build/agent-benchmarks/catalog-attribution-control/1787919021/summary.json` | `016865db` | off | bounded | 42.906 s | 42.906 s | 38.204 s |
| Full integrity, optimized | `build/agent-benchmarks/catalog-attribution-control/1787919604/summary.json` | `e5686f60` | on | full | 102.716 s | 47.316 s | 43.726 s |

The large first-sample spread (`42.906–104.329 s`) proves that reboot-to-reboot
filesystem/cache state dominates the full cold operation. It prevents using
the end-to-end first-sample difference as an FTS result.

## Phase evidence

Median fresh-builder totals across the 90 system shards were:

| Cohort | FTS optimize phase | FTS integrity phase | Source phase | Artifact publication |
|---|---:|---:|---:|---:|
| Bounded, optimized | 1.239 s | 0.107 s | 15.873 s | 18.218 s |
| Bounded, disabled optimize | 0.000 s | 0.124 s | 15.540 s | 17.892 s |
| Full integrity, optimized | 1.195 s | 1.669 s | 15.583 s | 19.584 s |

Disabling `optimize` removes about 1.24 s of measured FTS work, but the warm
end-to-end medians differed by only about 0.25 s host-observed and 0.35–0.46 s
runtime-observed, within the observed filesystem variance. It also produced a
different artifact byte stream (30,936,161 versus 32,742,497 copied bytes),
although the logical catalog fingerprint remained identical. Search-result
equivalence beyond the bounded build checks is not yet independently qualified.

The full integrity check adds about 1.56 s of direct phase work and did not
improve any refresh metric. Bounded integrity and optimized FTS therefore
remain the production defaults pending a dedicated search-correctness probe.

All cohorts retained more than 205 MiB on the `/tmp` filesystem and about 90 GB
on `/media/fat`; no storage failure occurred.
