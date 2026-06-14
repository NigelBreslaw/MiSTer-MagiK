# Arcade band-copy trial - 2026-06-08

This note records the arcade list framebuffer band-copy experiment and why it
was removed from the production path.

## Goal

The arcade list is now a Rust-rendered layer with a fixed selector overlay. That
made an obvious optimization tempting: instead of copying the whole list
viewport every scroll frame, shift the existing list pixels in `/dev/fb0`, then
copy only the newly exposed band plus selector/fade repair rectangles.

The experiment implemented that path with:

- `Display::scroll_rect_y(...)` to move the live framebuffer rectangle in place
- text-layer repair bands for newly exposed rows
- separate fade and selector overlay composition for repaired rectangles
- a gated default using `MISTER_ARCADE_FB_SCROLL_BLIT=1`

## Benchmark result

Raw rows are in `history/toolchain-bench/results.tsv`.

| Mode | FPS | Render | Copy | Rows | CPU mean | CPU max | Visual |
|---|---:|---:|---:|---:|---:|---:|---|
| Previous normal | 60 | 59 us | 2645 us | 384 | 26% | 65% | yes |
| Band-copy normal | 60 | 59 us | 6296 us | 528 | 45% | 70% | yes |
| Gated default normal | 60 | 63 us | 2742 us | 384 | 27% | 79% | yes |
| Previous turbo | 60 | 128 us | 2847 us | 385 | 28% | 66% | yes |
| Band-copy turbo | 60 | 114 us | 6409 us | 529 | 47% | 72% | yes |
| Gated default turbo | 60 | 124 us | 2797 us | 385 | 28% | 65% | yes |

The visual captures passed, but the performance result was clearly worse. The
band-copy path more than doubled copy time and raised CPU by roughly 19 points.

## Why it lost

`/dev/fb0` is write-combined and good for sequential writes from cached RAM. The
band-copy path used `copy_within` on the live framebuffer mapping, which requires
reading from that write-combined memory before writing it back. That is the wrong
memory access pattern for this hardware.

The row accounting also went in the wrong direction: once fade and selector
repair zones were included, the path reported around 528-529 touched rows versus
384-385 for the straightforward full-list copy.

## Decision

Removed from production code after the checkpoint commit. Keep the simple arcade
list copy path:

1. draw/scroll the cached arcade list layer in RAM
2. copy the full 384-row list viewport to `/dev/fb0`
3. draw fade and fixed selector as overlays

Do not reintroduce live framebuffer scroll/blit for this list unless a future
path avoids reading from `/dev/fb0`, for example by shifting a cached RAM
composite buffer and only writing the final dirty pixels to the framebuffer.

## Follow-up: scroll-present A/B - 2026-06-11

We re-tested the later `MISTER_ARCADE_SCROLL_PRESENT=1` path on the MiSTer after
reviewing whether the disabled path might actually be worth keeping. It was not.
The path scrolled portions of the already-presented arcade list in live
`/dev/fb0`, then patched the exposed bands and overlays. That kept the same bad
memory pattern as the original band-copy trial: read-modify-write against the
write-combined framebuffer.

Trace files are in `build/preview-scroll-profiles/`:

| Case | Path | Avg wall | P95 wall | Slow >16.7 ms | Slow >20 ms | Avg fb-present | P95 fb-present |
|---|---|---:|---:|---:|---:|---:|---:|
| list-only standalone | normal | 16428 us | 16522 us | 22 | 1 | 1806 us | 1998 us |
| list-only standalone | scroll-present | 16452 us | 16569 us | 24 | 1 | 4033 us | 4356 us |
| list-only real launcher | normal | 16422 us | 16570 us | 20 | 2 | 1686 us | 1957 us |
| list-only real launcher | scroll-present | 16437 us | 16562 us | 30 | 2 | 3820 us | 4323 us |
| preview standalone | normal | 16415 us | 17346 us | 158 | 2 | 2031 us | 2893 us |
| preview standalone | scroll-present | 16462 us | 17320 us | 163 | 4 | 3881 us | 4808 us |
| preview real launcher | normal | 16434 us | 17312 us | 161 | 3 | 1821 us | 2804 us |
| preview real launcher | scroll-present | 16459 us | 17329 us | 166 | 5 | 3823 us | 5022 us |

The wall-frame cadence remained near 60 Hz because vsync absorbs some of the
extra work, but the presentation cost roughly doubled and p95 framebuffer
present time reached about 5 ms with previews enabled. The slow path and its
`MISTER_ARCADE_SCROLL_PRESENT` / `--scroll-present` toggles were removed after
this run so future profiling cannot accidentally compare against it again.

## Fresh review baseline - 2026-06-14

Command:

```bash
scripts/profile-preview-scroll.sh 20 held-scroll LISTSCROLL-BEFORE-20260614 --skip-build --list-only --fb-format 565 --preview-format raw-rgb565 --visual-captures 0
```

Result:

| Case | Frames | Avg wall | P95 wall | P99 wall | Slow >16.7 ms | Slow >20 ms | Avg fb-present | P95 fb-present | Rows |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| list-only normal | 1202 | 16361 us | 16480 us | 17136 us | 22 | 1 | 2592 us | 2840 us | 384 |

This run reinforces the earlier conclusion. Wall time is the gating user-visible
metric, but it sits close to one vblank even when the app burns extra present
time. Use `fb_present_us` / `overlay_present_us` as the headroom metric for this
specific optimization; otherwise a change can look neutral at 60 Hz while making
the frame loop less robust. The existing `ArcadeListUpdate::Scroll` remains a
RAM-surface reuse signal inside `ArcadeListRenderer`, not a request to scroll the
already-presented framebuffer.

## Catalog startup projection - 2026-06-14

Change: schema 18 adds a materialized `launcher_catalog` table at
`library-refresh` / import time, after launcher filtering and MRA variant
collapse. Startup now reads that projection directly and falls back to the old
join for older databases.

| Run | Catalog load | First frame | Games |
|---|---:|---:|---:|
| `CATALOG-BEFORE-20260614` | 849 ms | 888 ms | 9524 |
| `CATALOG-AFTER-20260614` | 316 ms | 564 ms | 9524 |

The one-time schema rebuild on the MiSTer took `scan_us=75035650` and
`import_us=30467612`; that cost is paid by `library-refresh`, not by cached UI
launch. The steady scroll trace was noisier after the rebuild because preview
cache state changed, so the keep/revert decision is based on the startup timing
lines rather than the unrelated preview-frame tail.
