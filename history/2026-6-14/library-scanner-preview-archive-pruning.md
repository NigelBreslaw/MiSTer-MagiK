# Library scanner preview archive pruning - 2026-06-14

## Context

MiSTer MagiK runtime previews no longer decode or resize normal PNG/JPG
screenshots on the MiSTer. The launcher uses prebuilt raw565 preview archives,
with the raw pack preferred over the smaller LZ4 block archive when present.

The library scanner still had an older optional metadata pass that walked for
`gamelist.xml`, parsed screenshot paths, and checked image files. That made full
rebuilds slower and pulled screenshot/cache media directories into the
no-change manifest.

## Decision

The scanner no longer reads `gamelist.xml` or probes normal screenshot files for
preview metadata.

The scanner may read the compact preview archive index once, then use MRA
`<setname>` values as virtual preview keys only when that stem exists in the
archive. This preserves preview availability without touching
`/media/fat/_Arcade/media/screenshot` or per-image cache files during scans.

Ignored directory components now include:

- `screenshot`
- `screenshots`
- `screenshot-magik`
- `boxart`

## Benchmark

Device benchmark command:

```bash
scripts/bench-library.sh LIB-SCAN-BEFORE-20260614 --device --replace-label --iterations 3 --post-reboot
scripts/bench-library.sh LIB-SCAN-AFTER-20260614 --device --replace-label --iterations 3 --post-reboot
```

Results are appended to `history/toolchain-bench/results-library.tsv`.

| Scenario | Before avg | After avg | Notes |
|---|---:|---:|---|
| Cold scan | 44.05 s | 25.54 s | About 42% faster |
| No-change manifest | 17.74 s | 16.43 s | About 7% faster |
| Cached arcade load | 206 ms | 177 ms | SQLite projection/load |
| SQLite bytes | 42.42 MB | 41.51 MB | Final after DB keeps archive-keyed previews |
| Post-reboot refresh | 66.79 s | 66.93 s | Boot-time contention dominates this single sample |
| Settled refresh | not sampled | 15.78 s | Same DB after boot settles |

Final after DB preview counts:

- `games`: 10,089 total, 828 with `has_image=1`
- `launcher_catalog`: 9,523 total, 698 with `has_image=1`

## Follow-up

During the post-benchmark boot check, Main reported `launcher_active=true` with
`visible_owner="core"` and `fb_enabled=0` while Slint reported a healthy 60fps
launcher. If stock OSD/static animation is visible in that state, treat it as a
separate framebuffer/OSD handoff bug rather than expected steady-state launcher
behavior.
