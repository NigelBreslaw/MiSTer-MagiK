# Download Benchmark Production Publish Alignment

Item 9 aligns `media-bench-download` with the production screenshot pack publish
path and adds stage rows for item 10 attribution.

## Change

- `media-bench-download` now publishes downloaded packs with
  `publish_pack_file_with_progress`.
- The benchmark writes a benchmark-scoped media state file through the same
  atomic publish helper used by production state writes.
- Benchmark artifacts are hidden, label-scoped files in the asset directory and
  are cleaned after the run, so installed packs and production state are not
  overwritten.
- The download wrapper preserves `stage_tsv` rows in stdout/results output and
  removes old stage rows when `--replace-label` is used.

## Hardware Run

Command:

```bash
scripts/profile-screenshot-download.sh ITEM09-AFTER-download-production-publish --system neogeo --iterations 1 --replace-label
```

Key rows:

```text
stage_tsv label=ITEM09-AFTER-download-production-publish-01 suite_label=ITEM09-AFTER-download-production-publish benchmark=media-bench-download system=neogeo stage=publish_copy ms=2067 bytes=24283092 result=bench-ok detail=progress_events=96
stage_tsv label=ITEM09-AFTER-download-production-publish-01 suite_label=ITEM09-AFTER-download-production-publish benchmark=media-bench-download system=neogeo stage=publish_sync ms=0 bytes=24283092 result=bench-ok detail=progress_events=96
stage_tsv label=ITEM09-AFTER-download-production-publish-01 suite_label=ITEM09-AFTER-download-production-publish benchmark=media-bench-download system=neogeo stage=publish_rename ms=0 bytes=24283092 result=bench-ok detail=progress_events=96
stage_tsv label=ITEM09-AFTER-download-production-publish-01 suite_label=ITEM09-AFTER-download-production-publish benchmark=media-bench-download system=neogeo stage=publish_parent_sync ms=7 bytes=24283092 result=bench-ok detail=progress_events=96
stage_tsv label=ITEM09-AFTER-download-production-publish-01 suite_label=ITEM09-AFTER-download-production-publish benchmark=media-bench-download system=neogeo stage=state ms=27 bytes=1010 result=bench-ok detail=path=/media/fat/mister-magik/assets/.screenshot-media-state.json.bench-1855-1782324950503
stage_tsv label=ITEM09-AFTER-download-production-publish-01 suite_label=ITEM09-AFTER-download-production-publish benchmark=media-bench-download system=neogeo stage=cleanup ms=76 bytes=0 result=bench-ok detail=removed=3
screenshot_download_bench_tsv ITEM09-AFTER-download-production-publish-01 neogeo identity 24283092 24283092 6181 0 2179 1286 9650 31.43 20.13 "9850b1a4046515e4f0f5130994cd4f49" identity HIT bench-ok
```

## Metric

This commit is benchmark alignment rather than a production optimization.
Specific evidence for the next optimization pass is now available:

- `publish_copy=2067ms`
- `publish_sync=0ms`
- `publish_rename=0ms`
- `publish_parent_sync=7ms`
- `state=27ms`
- `cleanup=76ms`
- `save_ms=2179ms`

The previous benchmark path did not emit these production publish stages.
