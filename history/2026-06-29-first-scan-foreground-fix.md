# First-Scan Foreground Scheduling Fix - 2026-06-29

Problem: after `0a8428cc perf: centralize background thread policy`, the first
catalog scan missed the RAM catalog gate:

- Default policy before fix: `library_ready=44768ms`, gate `41000ms`.
- Diagnostic with `MISTER_THREAD_POLICY=off`: `library_ready=40403ms`.

Cause: the first database creation path inherited the same protective background
policy as warm maintenance work. `library-catalog` ran nice `5` on CPU0 and
`library-walker` ran nice `10` on CPU0, slowing scan/classify enough to consume
the tight first-scan headroom.

Fix: keep normal background policy for warm/background work, but switch the
staged RAM catalog build to foreground roles before first-build scan work:

- `catalog-foreground`: nice `0`, affinity `any`.
- `library-walker-foreground`: nice `0`, affinity `any`.

Verification:

```text
FIRSTSCAN-FOREGROUND-FIX-20260629
library_scan_complete 35472ms
library_ready         39705ms
catalog_us            4007511us
library_db_saved      47906ms
sqlite_publish        587ms
db_count              9237
```

Final commit-candidate verification after tidy and redeploy:

```text
FIRSTSCAN-FOREGROUND-FIX-COMMIT-20260629
library_scan_complete 34876ms
library_ready         39094ms
library_db_saved      47311ms
db_count              9237
```

Thread policy evidence from the device:

```text
thread=library-catalog role=catalog-worker intended_nice=5 actual_nice=5 affinity=cpu0
thread=library-catalog role=catalog-foreground intended_nice=0 actual_nice=0 affinity=any
thread=library-walker role=library-walker-foreground intended_nice=0 actual_nice=0 affinity=any
```

Conclusion: first database creation should be treated as a foreground bootstrap
job. The background CPU0/nice policy remains correct for warm maintenance,
preview prefetch, and media work, but it is too conservative for the initial
catalog build gate.
