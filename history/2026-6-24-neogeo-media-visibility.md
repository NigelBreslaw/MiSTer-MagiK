# Neo Geo Cold-Boot Media Visibility

Item 8 fixes the confirmed Neo Geo cold-boot download visibility bug.

## Confirmed Cause

The cold media benchmark from item 7 proved that Neo Geo was not lost in
discovery, catalog seeding, queueing, download progress, or terminal state.
Neo Geo entered the media progress model, but its first active progress happened
after the catalog scan popup had been hidden:

```text
ITEM07 neogeo discovered=1 ensured=1 queued=1 queue_started=1 progress_download_seen=1 ui_row_seen=1 ui_rendered_seen=0 terminal=done ui_issue=render_missing
```

Arcade and Saturn rendered because they downloaded while the catalog scan popup
was still visible.

## Change

- Added a standalone media progress popup for the launcher when
  `media-pack-progresses.length > 0` and the catalog scan popup is hidden.
- Updated `screenshot_media_ui_visibility` so `rendered=1` reflects either the
  catalog scan popup or the standalone media popup.
- Added a terminal progress hold clear when all requested packs have reached a
  terminal phase. This keeps the standalone popup from sticking if the worker
  `Done` message is delayed or missing.

## Benchmark

Before:

```bash
scripts/profile-media-cold-boot.sh ITEM07-AFTER-media-cold-boot --skip-build --replace-label --timeout 1200
```

After:

```bash
scripts/profile-media-cold-boot.sh ITEM08-AFTER-neogeo-media-visibility --deploy-device --replace-label --timeout 1200
```

Artifacts:

- `build/media-cold-boot/ITEM07-AFTER-media-cold-boot.log`
- `build/media-cold-boot/ITEM07-AFTER-media-cold-boot.report.tsv`
- `build/media-cold-boot/ITEM08-AFTER-neogeo-media-visibility.log`
- `build/media-cold-boot/ITEM08-AFTER-neogeo-media-visibility.report.tsv`
- `build/media-cold-boot/ITEM08-AFTER-neogeo-media-visibility-snapshot/fb0.png`
- `history/toolchain-bench/results-media-cold-boot.tsv`

## Result

Neo Geo visibility:

```text
BEFORE ui_rendered_seen=0 ui_issue=render_missing
AFTER  ui_rendered_seen=1 ui_issue=none
```

After row:

```text
media_cold_boot_tsv label=ITEM08-AFTER-neogeo-media-visibility system=neogeo discovered=1 ensured=1 queued=1 queue_started=1 progress_download_seen=1 ui_row_seen=1 ui_rendered_seen=1 terminal=done worker_done=0 completion=targets_terminal invalid_reason=ok ui_issue=none
```

The after snapshot artifact was `960x540` and nonblank.
