# ZIP Central Directory Buffering

Item 11 replaces per-entry ZIP central-directory reads/seeks with bounded
buffered reads in the production catalog scanner. The fix keeps ZIP64 location
handling, validates central-directory bounds without integer overflow, reads
normal-sized central directories into an 8 MiB capped buffer, and keeps a 64 KiB
streaming buffer for unusually large directories.

## Cause

`scan_zip_central_directory` already read the EOCD tail in one bounded read, but
then parsed each central-directory entry with separate 46-byte header reads,
entry-name reads, and seek-based metadata skips. On the MiSTer SD path this made
the archive TOC phase sensitive to many tiny operations even though no file
payload bytes are needed.

## Hardware Evidence

Immediate-parent binary:

```text
scripts/profile-library-io.sh ITEM11-BEFORE-zip-toc --replace-label --sample-limit 120

library_scan_timing archive_toc 26712us containers=154
scan_stage_archive_toc 27ms containers=154
scan_stage_classify_total 2727ms discoveries=9354 normal_files=7897 containers=154 entries=281
refresh_done scan_us=2875279 discover_us=1875838 classify_us=2727285 discoveries=9229 normal_files=7897 containers=154 entries=281
```

Candidate binary:

```text
scripts/profile-library-io.sh ITEM11-AFTER-zip-toc --replace-label --sample-limit 120

library_scan_timing archive_toc 9351us containers=154
scan_stage_archive_toc 9ms containers=154
scan_stage_classify_total 2503ms discoveries=9354 normal_files=7897 containers=154 entries=281
refresh_done scan_us=2646070 discover_us=1908766 classify_us=2503280 discoveries=9229 normal_files=7897 containers=154 entries=281
```

Result:

- `scan_stage_archive_toc`: `26.712ms -> 9.351ms`, a `64.99%` reduction.
- Rounded TSV row: `27ms -> 9ms`, a `66.67%` reduction.
- Counts unchanged: `discoveries=9229`, `normal_files=7897`,
  `containers=154`, `entries=281`.

Cold first-scan production run:

```text
scripts/profile-first-scan.sh ITEM11-BEFORE-zip-toc --skip-build --replace-label --timeout 240
scripts/profile-first-scan.sh ITEM11-AFTER-zip-toc --deploy-device --replace-label --timeout 240
```

The cold first-scan row moved only `708ms -> 693ms`; that path is dominated by
cold SD/file-open behavior around the scan. Counts stayed unchanged:
`discoveries=9229`, `normal_files=7897`, `containers=154`, `entries=281`,
`db_count=9229`.

## Validation

```text
cargo test --manifest-path magik-gui/catalog/Cargo.toml zip_central_directory -- --nocapture
```

The new regression case covers central-directory extra/comment padding larger
than the skip scratch buffer.

