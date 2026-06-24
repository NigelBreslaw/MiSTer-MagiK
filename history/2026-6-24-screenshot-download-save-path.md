# Screenshot Download Save Path

Item 10 checks the Neo Geo screenshot download save path against the direct
production save path. The suspected ~8.3s save phase was not a FAT32/exFAT
rename or sync problem; it came from the download benchmark using a different
save path than production before `media-bench-download` was aligned with
`publish_pack_file_with_progress`.

## Before

Immediate-parent baseline context from the previous benchmark path:

```text
screenshot_download_bench_tsv PERFREVIEW-20260624-SHOT-DL-01 neogeo identity ... download_ms=6066 save_ms=8314 verify_ms=1323 total_ms=15704 result=bench-ok
```

Direct save comparison on current hardware:

```text
scripts/profile-screenshot-save.sh ITEM10-DIRECT-SAVE-neogeo --system neogeo --iterations 5 --replace-label

iteration total_ms
1         1831
2         1961
3         1923
4         1985
5         2197
```

Direct save p95/max for the five-run gate is therefore `2197ms`; the allowed
download save threshold is `2746ms` (`2197 * 1.25`, rounded down).

## After

```text
scripts/profile-screenshot-download.sh ITEM10-AFTER-save-path-gated --system neogeo --iterations 1 --replace-label --max-save-ms 2746
```

Key rows:

```text
stage_tsv label=ITEM10-AFTER-save-path-gated-01 stage=publish_copy ms=2100 bytes=24283092 result=bench-ok detail=progress_events=96
stage_tsv label=ITEM10-AFTER-save-path-gated-01 stage=publish_sync ms=0 bytes=24283092 result=bench-ok
stage_tsv label=ITEM10-AFTER-save-path-gated-01 stage=publish_rename ms=0 bytes=24283092 result=bench-ok
stage_tsv label=ITEM10-AFTER-save-path-gated-01 stage=publish_parent_sync ms=7 bytes=24283092 result=bench-ok
stage_tsv label=ITEM10-AFTER-save-path-gated-01 stage=state ms=27 bytes=1010 result=bench-ok
stage_tsv label=ITEM10-AFTER-save-path-gated-01 stage=cleanup ms=76 bytes=0 result=bench-ok
screenshot_download_bench_tsv ITEM10-AFTER-save-path-gated-01 neogeo identity 24283092 24283092 5897 0 2215 1285 9400 32.94 20.67 "9850b1a4046515e4f0f5130994cd4f49" identity HIT bench-ok
metric_tsv label=ITEM10-AFTER-save-path-gated system=neogeo metric=screenshot_download_save_ms_max value=2215 unit=ms valid=1
validity_tsv label=ITEM10-AFTER-save-path-gated valid=1 invalid_reason=ok detail=max_save_ms=2215 limit_ms=2746 rows=1 systems=neogeo
```

Result:

- Save path improved from `8314ms` to `2215ms` (`73.36%` lower).
- Total wall time improved from `15704ms` to `9400ms` (`40.14%` lower).
- Download save path is within the direct-save threshold: `2215ms <= 2746ms`.
- Dominant remaining save phase is expected data copy (`publish_copy=2100ms`);
  sync, rename, and parent sync are not the slow subphase.

