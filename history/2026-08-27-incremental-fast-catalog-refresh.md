# Incremental fast-catalog refresh evidence

Date: 2026-08-27

Branch: `nigel/arcade-catalog-prototype`

## Retained design

- Nine independently sourced systems; legacy source inputs reported as zero.
- Per-system checksummed watch and row snapshots with two-slot manifests.
- Metadata-only no-change planning and two bounded checking lanes.
- Per-system SQLite search shards and NavPack instant-navigation artifacts.
- Manifest-last changed-system publication; unchanged immutable artifacts are
  reused.
- Explicit launcher update with one terminal event per system and determinate
  `Updating systems X/N` progress.
- Reboot-stable exFAT comparison uses path, size, and mtime. Unstable inode and
  ctime observations are diagnostic only.

## Reboot-cold scratch build

Evidence: `build/agent-benchmarks/fast-refresh/initial-independent-amiga-recursive.json`

| Phase | Time |
|---|---:|
| Independent source discovery | 13.94 s |
| SQLite/NavPack publication | 3.54 s |
| Refresh-state capture/publication | 1.14 s |
| Combined measured phases | 18.61 s |

The build produced 8,760 rows and nine systems with no legacy inputs. Arcade
used Update_All metadata for known MRAs and validated installed ROM/core
requirements; supplemental MRAs still use direct parsing. C64 visited only the
2,295 OneLoad64 source entries rather than the broader personal C64 tree.

Amiga produced one row. Recursive inspection found no installed
AmigaVision/MegaAGS HDF and listing pair under `/media/fat/games/Amiga`; the
adapter therefore retained only the filesystem fallback instead of publishing
unverified prepared rows.

## Incremental measurements

| Scenario | Total | Plan | Rescan | Artifacts | Snapshot | Result |
|---|---:|---:|---:|---:|---:|---|
| No change, sequential | 4.50 s | 4.50 s | 0 | 0 | <1 ms | 9 unchanged |
| No change, two lanes | 4.17 s | 4.17 s | 0 | 0 | <1 ms | 9 unchanged |
| Add one SNES ROM | 6.29 s | 3.22 s | 1.24 s | 1.41 s | 0.41 s | 1 updated, 8 unchanged |

The two-lane planner was retained because it reduced cold wall time by 7.3%.
The no-change run opened zero row snapshots and wrote zero artifacts. The SNES
mutation published one changed system only and increased the canonical count
from 8,760 to 8,761; cleanup reconciled the isolated fixture afterward.

## Targets not met

The measured no-change result is 4.17 seconds rather than 0.5 seconds. Cold
exFAT watch-index and metadata reads dominate. The changed direct-ROM result is
6.29 seconds rather than 1 second because the current reconciler rebuilds the
complete changed system and republishes its SQLite/NavPack pair. These are
measured prototype limits, not qualified release claims.

An attempted broad Arcade watch traversed 562 directories and yielded a 5.38
second profiled plan. Restricting known Update_All content to its updater index
and watching only supplemental installation anchors reduced the complete watch
set from 649 directories to 92. A first cold profile also proved exFAT inode and
ctime unstable across reboot, so those fields were rejected as authority.

## Evidence files

- `build/agent-benchmarks/fast-refresh/no-change-profile-1/`
- `build/agent-benchmarks/fast-refresh/no-change-profile-2/`
- `build/agent-benchmarks/fast-refresh/no-change-profile-3/`
- `build/agent-benchmarks/fast-refresh/no-change-unprofiled-1.json`
- `build/agent-benchmarks/fast-refresh/no-change-unprofiled-2lane.json`
- `build/agent-benchmarks/fast-refresh/add-snes-unprofiled.json`

The build evidence directory is local and intentionally untracked. This history
record preserves the durable conclusions without adding generated profiles or
device data to Git.
