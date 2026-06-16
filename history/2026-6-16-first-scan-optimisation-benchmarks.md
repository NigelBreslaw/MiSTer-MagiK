# 2026-06-16 First-Scan Optimisation Benchmarks

Device: MiSTer at `192.168.1.117`, production `MiSTer_MagiK` supervised boot.

## First-Scan UI Path

`scripts/profile-first-scan.sh` measures reset/first-boot behavior by deleting
the production catalog database, rebooting, and waiting for the launcher catalog
sync event.

| Label | Commit | First frame | Ready/sync | Scan | Import | Games |
|---|---:|---:|---:|---:|---:|---:|
| `FIRSTSCAN-BASE-20260616` | `bbea05a` | 1.622s | 103.117s | 70.109s | 28.398s | 9959 |
| `FIRSTSCAN-UIFPS-20260616` | `80775c4` | 1.604s | 102.795s | 70.615s | 27.654s | 9959 |
| `FIRSTSCAN-TMPFS-20260616` | `f751185` | 1.616s | 100.702s | 70.318s | 25.820s | 9959 |
| `FIRSTSCAN-RAMCAT-20260616` | `c7b6b8c9` | 1.596s | 74.518s usable / 102.903s reconciled | 69.934s | 27.790s | 9959 |

The redraw/progress throttling is effectively neutral for total first-scan time
but reduces unnecessary UI work. The tmpfs SQLite build path improved this
single full first-scan run by about 2.1s, not enough to enable by default under
the 10% full-run threshold.

The RAM-first catalog path makes Home usable as soon as the scan can produce an
in-memory launcher catalog. On this run the temporary RAM catalog had 9393 rows;
after SQLite persistence completed, the launcher reconciled to the durable
materialized catalog with 8389 UI rows. The persisted database still contained
9959 games.

## Library Scanner Bench

`LIBSCAN-BASE-20260616` after fixing the benchmark script to use supervisor
suspend/resume:

- Cold scan: 22.660s, 11.979s, 11.785s; median 11.979s.
- Import: 24.671s, 23.950s, 26.265s; median 24.671s.
- Post-reboot no-change refresh: 32.814s.

`LIBSCAN-PRECOUNT-20260616`:

- Pre-count discovery: 9.485s, 9.495s, 9.501s; median 9.495s.
- This is the extra cost of a determinate percent from the very beginning, so
  discovery should remain indeterminate.

`LIBSCAN-TMPFS-20260616` with `--sqlite-build-dir /tmp`:

- Cold scan median: 11.636s.
- Import median: 18.431s.
- Isolated import improved by about 6.24s, but full first-scan improvement did
  not clear the default-enable threshold.

## I/O Profile

`LIBIO-BASE-20260616`:

- Cold scan: 12.945s.
- Import: 24.786s.
- Process write bytes reached about 32.1 MB.
- `mmcblk0` disk I/O time rose by about 20.35s during the run.

`LIBIO-TMPFS-20260616` with `--sqlite-build-dir /tmp`:

- Cold scan: 12.989s.
- Import: 18.724s.
- `mmcblk0` disk I/O time rose by about 10.36s during the run.

The I/O profile supports the earlier conclusion: scan/classification is mostly
CPU/filesystem traversal, while SQLite import is meaningfully SD/exFAT/FUSE
bound. Building SQLite in tmpfs cuts the isolated import cost, but RAM-first
catalog display gives the larger user-visible win.

## OSD Safety Finding

The old `scripts/bench-library.sh` directly killed `mister-magik-fb` before
deploying/running the CLI benchmark. That bypassed the `MiSTer_MagiK`
supervisor and could leave display/OSD state out of sync. The script now uses
`mister_magik_suspend`/`mister_magik_resume` through `/dev/MiSTer_cmd` for
deploy and benchmark CLI execution.
