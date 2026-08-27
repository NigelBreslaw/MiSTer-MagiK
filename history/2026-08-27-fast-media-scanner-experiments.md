# Fast media scanner experiments

Date: 2026-08-27

The independent fast-catalog prototype was extended with focused PlayStation,
BBC Micro, and MSX scanners. These systems exercise optical-media support
filtering, a large mixed disk/tape directory, and a simple cartridge control.
No legacy catalog snapshot, scanner cache, SQLite database, or sidecar was read.

The real installed corpus contained 5,297 files and produced 5,259 canonical
game rows:

| System | Files | Games | Ignored | Unmatched |
|---|---:|---:|---:|---:|
| PlayStation | 123 | 118 | 5 | 0 |
| BBC Micro | 4,764 | 4,734 | 28 | 2 |
| MSX | 410 | 407 | 3 | 0 |

PSX BIN/IMG track dependencies, BIOS ROMs, and SBI support material are
explicitly non-playable. A fixture proves those files cannot become rows. The
real PSX collection was CHD-based and contained no BIN/IMG dependency files.

## Experiment 1: borrowed rules and unsorted traversal

Evidence: `build/agent-benchmarks/fast-refresh/media-psx-bbc-msx-ab.json`

| Implementation | Cold total | Result |
|---|---:|---|
| Existing sorted, owned-rule walker | 3.145 s | Baseline |
| Borrowed-rule, unsorted walker | 3.219 s | 2.3% slower cold |

The candidate was faster after metadata became warm, but BBC Micro's cold exFAT
enumeration dominated and regressed. The unsorted `read_dir` candidate was not
retained as the optimized backend.

## Experiment 2: bounded fd-relative namespace walk

Evidence:
`build/agent-benchmarks/fast-refresh/media-psx-bbc-msx-namespace-ab.json`

| System | Baseline cold | Namespace cold | Result |
|---|---:|---:|---|
| PlayStation | 0.279 s | 0.207 s | 25.8% faster |
| BBC Micro | 2.440 s | 2.384 s | 2.3% faster |
| MSX | 0.137 s | 0.138 s | Effectively flat |
| **Total** | **2.961 s** | **2.875 s** | **2.9% faster** |

The retained Linux backend read the PSX directory in four calls/6.936 KB, BBC
Micro in four calls/360.408 KB, and MSX in two calls/20.144 KB. It required no
per-entry type-stat fallback. Warm total time fell from 0.691 s to 0.436 s.

## Experiment 3: bounded two-lane scheduling

Evidence:
`build/agent-benchmarks/fast-refresh/media-psx-bbc-msx-parallel-ab.json`

The final candidate assigns BBC Micro to one lane and scans PSX then MSX on the
other. It deliberately uses only the two available Cortex-A9 cores.

| Implementation | Cold total | Result |
|---|---:|---|
| Existing sequential baseline | 3.135 s | Baseline |
| Two-lane fd-relative scanner | 2.638 s | 15.8% faster |

The two-lane result was 8.2% faster than the separately measured sequential
fd-relative result. Parallel SD access made each small scan slower in isolation,
but useful overlap remained. Warm two-lane time was slower than the sequential
fd-relative path, so this scheduling policy is appropriate for cold scratch
construction rather than the ordinary no-change refresh path.

Both alternating-order reboot samples had exact launch-reference and complete
row parity for all three systems, zero read/archive errors, and left the
production registry unchanged. The benchmarked binary SHA-256 was
`09f527860acc5dff30a27e7f9b77efe5defb5bcd0dc4e462aaf9079ff4a4d230`.

## Cutover findings

The experiment intentionally preserves current profile classification, so the
36 ignored and two unmatched files need a named-path audit before these systems
become authoritative in the UI. Exact parity proves the optimization is safe;
it does not by itself prove every existing support-file heuristic is desirable.
After that audit, the retained scanner can be connected to fast snapshots,
incremental refresh, SQLite, NavPack, and the real registry as three additional
systems.
