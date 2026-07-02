# Filesystem Reset-Fault Testing

This test path intentionally interrupts MagiK filesystem writes with Main's
fast reset-manager path:

```text
mister_magik_direct_reset_no_sync
```

Use it only as an attended destructive experiment. The catalog database,
catalog projections, screenshot packs, screenshot indexes, screenshot media
state, and their temporary files are disposable during these tests. The runner
removes them freely and recovers by rebuilding the library database and letting
runtime media redownload later.

Settings are different: malformed persistent settings after a reset are a real
bug. A test may observe old settings, new settings, missing settings, or default
fallback behavior, but it should not leave unreadable state that blocks launcher
startup.

## Runner

```bash
scripts/device-fs-fault-reset.sh FSFAULT-YYYYMMDD --scenario catalog --iterations 3
scripts/device-fs-fault-reset.sh FSFAULT-YYYYMMDD --scenario projections --iterations 3
scripts/device-fs-fault-reset.sh FSFAULT-YYYYMMDD --scenario media --iterations 3
scripts/device-fs-fault-reset.sh FSFAULT-YYYYMMDD --scenario settings-marker --iterations 5
scripts/device-fs-fault-reset.sh FSFAULT-YYYYMMDD --scenario reset-delete --iterations 3
scripts/device-catalog-acceptance.sh --settle 5
```

`--scenario all` runs every scenario. Results append to:

```text
history/toolchain-bench/results-fs-fault-reset.tsv
```

Each row records the fault point, trigger path, whether the host observed the
device go down, launcher readiness, DB query health, media-state health,
catalog acceptance, and notes.

## Fault Interface

MagiK checks these environment variables at instrumented write boundaries:

```text
MISTER_FS_FAULT_POINT=<point-name>
MISTER_FS_FAULT_ACTION=direct-reset-no-sync
MISTER_FS_FAULT_DELAY_MS=2000
```

When the configured point matches, MagiK writes:

```text
/tmp/mister-magik/fs-fault.json
```

Then it sends `mister_magik_direct_reset_no_sync` to `/dev/MiSTer_cmd` and
sleeps briefly so reset can take the device down.

## Fault Points

Catalog SQLite publish:

```text
catalog.sqlite.after_build_temp_sync
catalog.sqlite.after_final_temp_copy
catalog.sqlite.after_final_temp_sync
catalog.sqlite.after_rename_before_parent_sync
```

Catalog projections:

```text
catalog.summary.after_temp_write
catalog.summary.after_temp_sync
catalog.summary.after_rename_before_parent_sync
catalog.navigation.after_temp_write
catalog.navigation.after_temp_sync
catalog.navigation.after_rename_before_parent_sync
```

Screenshot media:

```text
media.pack.after_temp_write
media.pack.after_temp_sync
media.pack.after_rename_before_parent_sync
media.index.after_temp_write
media.index.after_temp_sync
media.index.after_rename_before_parent_sync
media.state.after_temp_write
media.state.after_temp_sync
media.state.after_rename_before_parent_sync
```

Small persistent and launch-support writes:

```text
settings.after_temp_write
settings.after_rename
launcher.rebuild_marker.after_write
button_overrides.after_temp_write
button_overrides.after_temp_sync
button_overrides.after_rename
button_overrides.after_remove
amigavision_descriptor.after_temp_write
amigavision_descriptor.after_temp_sync
amigavision_descriptor.after_rename_before_parent_sync
crash_report.report.after_temp_sync
crash_report.report.after_rename
crash_report.latest.after_temp_sync
crash_report.latest.after_rename
```

Reset/delete flow:

```text
reset_delete.database.after_remove
reset_delete.summary.after_remove
reset_delete.navigation.after_remove
reset_delete.screenshot_asset.after_remove
```

## Interpreting Failures

Good outcomes:

- Old valid artifact remains.
- New valid artifact is installed.
- Disposable catalog/media artifact is missing and recovery rebuilds or
  redownloads it.
- Launcher reaches a single active process and catalog acceptance passes.

Failure candidates for later mitigation:

- Zero-byte or malformed `settings.json`.
- A persistent marker with unexpected contents.
- Launcher cannot restart after cleanup.
- `library-refresh` cannot rebuild after disposable catalog/media removal.
- `scripts/mister db "SELECT count(*) FROM launcher_catalog;"` fails after
  recovery.
- Screenshot media state exists but is not valid JSON-shaped state.

Do not infer a mitigation from one failed row. Re-run the specific point first;
this harness is meant to rank real reset windows before changing write policy.
