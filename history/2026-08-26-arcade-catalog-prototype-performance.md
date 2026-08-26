# Arcade catalog prototype performance review — 2026-08-26

## Outcome

A standalone Arcade-only builder was implemented without importing the legacy
Catalog V3 scanner or database writer. On the measured MiSTer corpus it creates
1,181 active variant records representing 925 preferred families in a
149,977-byte catalog. Every compared arm produced byte-identical active output.

The strongest complete from-Update_All cold result is 2.311 seconds. The
production-shaped path, where Update_All knowledge is compiled ahead of boot
and only the card-specific active catalog is created after reboot, measured
2.176–2.670 seconds single-threaded in the first two repeated pairs. Retained
legacy cold evidence reaches its first Arcade system 8.933–9.575 seconds after
builder execution begins. The prototype is therefore approximately 3.3–4.4x
faster depending on which valid cold sample and boundary are compared.

This is a directional comparison, not schema parity. The legacy builder is
performing broader Catalog V3 discovery and publication, while the prototype
builds only Arcade and omits SQLite search, NavPack, registry, scanner cache,
resume state, and non-Arcade systems.

## Scope and method

Only production code and the production Arcade corpus were included. The
experimental effects paths were excluded. Research covered the production
catalog pipeline, Update_All index, Arcade ROM eligibility, exFAT behavior,
Cortex-A9 concurrency, publication, and existing benchmark evidence.

Every authoritative prototype timing was taken by the typed
`arcade-catalog-prototype-cold` scenario. Each arm:

1. verifies the coherent Dev platform and exact focused binary receipt;
2. performs a supervised reboot and proves that the boot ID changed;
3. waits for launcher health and suspends through acknowledged Main control;
4. removes the arm output, syncs, and writes `3` to `drop_caches`;
5. creates the active catalog from scratch;
6. downloads the output/report, resumes the launcher, and verifies health.

Parallel and single-thread arms use separate reboots. No warm rerun is included
as timing authority. The immutable source-base test compiles that base before
the reboot; the active catalog itself does not exist until after reboot and the
base is cold after reboot/cache drop.

Measured corpus:

- 3,069 Update_All Arcade rows;
- 3,067 rows with enriched catalog metadata;
- 2 derived rows;
- 11 ambiguous primary-ROM rows;
- 2,376 MAME ZIP names and 27 HBMAME ZIP names;
- 1,181 playable active records;
- 925 preferred families.

## Phase-one review findings

The legacy cold path paid for a general filesystem walk, per-MRA discovery and
classification, broad catalog construction, and multi-artifact publication.
The retained 2026-08-24 cold captures show:

| Legacy cold evidence | Builder mode | First Arcade discovery | Delta |
|---|---:|---:|---:|
| `cold-boot/1787508360` | 1.878200 s | 10.811628 s | 8.933428 s |
| `cold-boot/1787344344` | 1.090538 s | 10.665220 s | 9.574682 s |

In the first run, the first-visible scan alone reported 3.564 seconds of
discovery and 0.378 seconds of classification before a 0.853-second preparation
stage. First-visible readiness was reached at 12.146 seconds after process
startup, 10.267 seconds after builder mode began.

The retained whole-catalog `catalog-full-build-rebuild/1787306191` evidence is
not a cold Arcade-only comparator. It took 186.136 seconds to complete all
systems and 9.896 seconds to first visibility. Once its general scan had already
materialized Arcade inputs, its isolated Arcade projection took 522.633 ms to
build 968 games and 255.738 ms to publish 1,120,030 bytes. That evidence shows
that projection is not the dominant cold cost; discovery is.

The phase-one design conclusion was therefore to remove discovery work instead
of micro-optimizing SQLite:

- compile stable Update_All metadata before boot;
- enumerate four ROM directories shallowly and never open ZIPs;
- eliminate impossible candidates from ROM names;
- group likely MRA paths by directory and enumerate each directory once;
- trust indexed metadata for present paths in fast mode;
- fail ambiguous rows closed without opening them;
- retain a full-walk recovery mode for unindexed/custom content;
- publish one compact binary atomically.

## Optimization chronology

All rows below are reboot-cold, from-scratch `build` controls unless marked
active-only. The two values are separate reboot arms.

| Revision/design | Parallel | Single | Result |
|---|---:|---:|---|
| Full walk plus path/size validation | 7.209 s | 5.968 s | Per-entry exFAT metadata dominated; two workers hurt. |
| Full walk, path authoritative | 5.014 s | 4.202 s | Removing redundant size metadata saved about 1.8 s single-threaded. |
| Extension-first full walk | 4.679 s | 4.221 s | Traversal itself remained dominant. |
| Individual Update_All path probes | 4.039 s | 3.878 s | Fewer paths helped little because each expected file still caused a lookup. |
| Directory-batched Update_All probes | **2.311 s** | 3.880 s | Same 1,181 records; parallel arm won this pair strongly. |
| Precompiled base, active-only pair 1 | 3.074 s | **2.670 s** | Parallel ordering reversed after a fresh reboot. |
| Precompiled base, active-only pair 2 | 2.646 s | **2.176 s** | Single worker repeated the production-path win. |

The directory-batched complete build reduced the initial 7.209-second parallel
control by 3.12x. The best single-thread active build reduced the 5.968-second
initial single-thread control by 2.74x.

## Where the final time goes

Across the two precompiled-base pairs:

- base decode: 54–59 ms;
- shallow ROM inventory: 719–769 ms;
- directory-batched MRA discovery: 1.145–1.465 s single-threaded;
- join/fallback before ambiguous-row elimination: 69–184 ms in the two best
  single-thread runs;
- deterministic selection: 13–17 ms;
- active atomic write: 22–41 ms.

The remaining dominant cost is exFAT directory enumeration, followed by the
shallow ROM inventory. CPU-side selection and compact serialization are already
small. The final code eliminates the 11 known-ambiguous XML reads from fast
mode; a final exact-commit cold pair is recorded separately when available.

## Dual-core conclusion

The Cortex-A9 result is workload-specific. Two directory workers produced one
excellent complete-build sample, but both repeated active-only pairs were
slower in parallel: 15% slower in pair 1 and 22% slower in pair 2. Earlier
full-walk and individual-probe controls also favored one worker. The exFAT card
has enough boot-to-boot latency variance that one favorable pair cannot justify
a parallel production default.

The selected production policy is one discovery worker. It gives the more
repeatable cold result and leaves the other core available to Main and the GUI.
`--parallel-probe` remains available for controlled profiling. Parallelism is
valuable only where the work is CPU-local or demonstrably independent; adding
threads to metadata-heavy exFAT access is not inherently an optimization.

## Size and capability comparison

| Artifact set | Bytes | Included capability |
|---|---:|---|
| Prototype source base | 703,617 | Ahead-of-time Update_All knowledge |
| Prototype active catalog | 149,977 | Arcade records, family/variant preference, launch metadata |
| Prototype base + active | 853,594 | Both prototype phases |
| Legacy Arcade SQLite + navigation + NavPack | 1,120,030 | Production Catalog V3 Arcade artifacts |

The active prototype is 7.47x smaller than the three legacy Arcade artifacts;
even base plus active is 24% smaller. This is not compression parity because
the prototype intentionally omits production features. It demonstrates that a
small boot-facing Arcade projection is practical, not that Catalog V3 can be
replaced byte-for-byte.

## Recommendation

Keep the prototype isolated until semantic parity is proven. The promotion path
with the best measured economics is:

1. generate and distribute the immutable source base with Update_All/database
   assets rather than compiling it during boot;
2. create only the active binary after reboot using one discovery worker;
3. expose the active format through a bounded launcher reader or translate it
   into the minimum existing Arcade navigation contract;
4. preserve atomic publication and fail-closed checksum/path validation;
5. run parity against launch paths, preferred families, variants, metadata,
   custom MRAs, Update_All version skew, and interrupted publication;
6. retain full-walk as an explicit completeness/recovery operation, not the
   first-visible hot path.

Evidence directories used by this report:

- `build/agent-benchmarks/arcade-catalog-prototype-cold/1787725572`
- `build/agent-benchmarks/arcade-catalog-prototype-cold/1787725971`
- `build/agent-benchmarks/arcade-catalog-prototype-cold/1787726340`
- `build/agent-benchmarks/arcade-catalog-prototype-cold/1787726906`
- `build/agent-benchmarks/arcade-catalog-prototype-cold/1787727327`
- `build/agent-benchmarks/arcade-catalog-prototype-cold/1787727656`
- `build/agent-benchmarks/arcade-catalog-prototype-cold/1787727779`
- `build/agent-benchmarks/cold-boot/1787508360`
- `build/agent-benchmarks/cold-boot/1787344344`
- `build/agent-benchmarks/catalog-full-build-rebuild/1787306191`
