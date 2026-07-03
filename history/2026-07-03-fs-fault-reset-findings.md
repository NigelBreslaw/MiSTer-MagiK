# Filesystem Reset-Fault Findings - 2026-07-03

Label: `FSFAULT-20260703`

This pass completed the original destructive reset-fault matrix for the main
filesystem write surfaces. Every committed row in this label observed the device
go down after `mister_magik_direct_reset_no_sync`, recovered the launcher, passed
the DB/media-state health checks, and passed catalog acceptance.

## Matrix Result

| Scenario | Fault points | Iterations | Rows | Result |
| --- | ---: | ---: | ---: | --- |
| `catalog` | 4 | 3 | 12 | ok |
| `projections` | 6 | 3 | 18 | ok |
| `settings-marker` | 3 | 5 | 15 | ok |
| `media` | 9 | 3 | 27 | ok |
| `reset-delete` | 4 | 3 | 12 | ok |

Total: 26 fault points, 84 rows, 0 bad rows.

The rows were recorded across these code points:

- `c666b96d`: catalog, projection, and settings-marker evidence.
- `461f3aaf`: media evidence after deterministic media trigger repair.
- `8df3bd5c`: reset-delete evidence.

Final post-matrix acceptance also passed:

```bash
scripts/device-catalog-acceptance.sh --settle 5
```

The final stale arming-file check returned no files:

```bash
scripts/mister run "ls -l /media/fat/mister-magik/launcher.env /tmp/mister-magik/fs-fault* /media/fat/mister-magik/rebuild-on-next-boot 2>/dev/null || true"
```

## Main Findings

- Catalog SQLite publish recovered cleanly at temp sync, final temp copy, final
  temp sync, and rename-before-parent-sync boundaries.
- Summary and navigation projections recovered cleanly at temp write, temp sync,
  and rename-before-parent-sync boundaries.
- Settings writes and the rebuild-on-next-boot marker recovered cleanly in this
  matrix. No malformed persistent settings state was observed.
- Screenshot pack, sidecar index, and media-state publish recovered cleanly after
  deterministic artifact-trigger coverage was added.
- Reset/delete cleanup recovered cleanly for database, summary projection,
  navigation projection, and screenshot asset removal points.

No mitigation candidate is justified by the completed `FSFAULT-20260703` matrix.
The disposable catalog and media artifacts behaved as intended: cleanup and
rebuild/redownload recovery restored a healthy launcher/catalog state.

## Historical Inconclusive Rows

Older `FSFAULT-20260702` rows with `down_seen=0` are not recovery evidence. They
only prove that the test harness did not observe a reset at those points.

Affected historical rows:

- `reset-delete` through `launcher-reset-delete-input`: 4 rows.
- `media` through the old pack/index trigger paths: 4 rows.

The media harness issue was fixed by routing media index and media-state points
through deterministic `media-bench-save --artifact index|state` publishes. The
reset-delete harness already used deterministic delete subcommands by the final
pass.

## Out Of Scope

The lower-priority fault points remain instrumented but were not exercised in
this pass:

- `button_overrides.*`
- `amigavision_descriptor.*`
- `crash_report.*`

They should get their own runner scenarios before drawing conclusions about
launch-preparation or crash-report durability.
