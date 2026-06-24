# Media Cold-Boot Observability

Item 7 adds a production cold media UI repro and observability path.

## Change

- Added `scripts/profile-media-cold-boot.sh`.
- The script defaults to a true cold catalog/media run by removing
  `library.sqlite3`, `library.summary.json`, and a label-scoped media asset
  directory before reboot.
- It emits `run_context_tsv`, `artifact_tsv`, `validity_tsv`,
  `media_cold_boot_tsv`, and focused `metric_tsv` rows.
- It captures the launcher log, `scripts/mister status`, and a framebuffer
  snapshot. Snapshot PNGs are checked for dimensions and nonblank content.
- The launcher now logs media UI row visibility:
  `screenshot_media_ui_visibility`.
- The media worker now logs queue depth and start state:
  `screenshot_media_system_queued pending=...` and
  `screenshot_media_system_start`.
- Catalog media seeding now logs `screenshot_media_catalog_system_present`
  before each ensure, so cached and cold catalog paths can both prove catalog
  presence.

## Hardware Run

Command:

```bash
scripts/profile-media-cold-boot.sh ITEM07-AFTER-media-cold-boot --skip-build --replace-label --timeout 1200
```

Artifacts:

- `build/media-cold-boot/ITEM07-AFTER-media-cold-boot.log`
- `build/media-cold-boot/ITEM07-AFTER-media-cold-boot.status.txt`
- `build/media-cold-boot/ITEM07-AFTER-media-cold-boot.report.tsv`
- `build/media-cold-boot/ITEM07-AFTER-media-cold-boot-snapshot/fb0.png`
- `history/toolchain-bench/results-media-cold-boot.tsv`

The framebuffer snapshot was `960x540` and nonblank.

## Key Rows

```text
media_cold_boot_tsv label=ITEM07-AFTER-media-cold-boot system=arcade discovered=1 ensured=1 queued=1 queue_started=1 progress_download_seen=1 ui_row_seen=1 ui_rendered_seen=1 terminal=done worker_done=0 completion=targets_terminal invalid_reason=ok ui_issue=none
media_cold_boot_tsv label=ITEM07-AFTER-media-cold-boot system=saturn discovered=1 ensured=1 queued=1 queue_started=1 progress_download_seen=1 ui_row_seen=1 ui_rendered_seen=1 terminal=done worker_done=0 completion=targets_terminal invalid_reason=ok ui_issue=none
media_cold_boot_tsv label=ITEM07-AFTER-media-cold-boot system=neogeo discovered=1 ensured=1 queued=1 queue_started=1 progress_download_seen=1 ui_row_seen=1 ui_rendered_seen=0 terminal=done worker_done=0 completion=targets_terminal invalid_reason=ok ui_issue=render_missing
```

## Item 8 Lead

Neo Geo is not lost at discovery, ensure, queue, start, progress, or terminal
download. The row enters the media progress model (`ui_row_seen=1`) but is not
rendered (`ui_rendered_seen=0`) because its download starts after the catalog
scan popup has been hidden. Arcade and Saturn both render before that happens.

The worker-level `screenshot_media_update_done` row did not appear before all
target systems reached terminal state, so the script records
`completion=targets_terminal` and keeps `worker_done=0` visible.
