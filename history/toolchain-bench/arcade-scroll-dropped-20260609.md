# Dropped Arcade Scroll Experiments - 2026-06-09

## PR 3: Incremental Scroll Present via Framebuffer Scroll

Rejected.

The experiment replaced full arcade-list overlay presentation during
`arcade_page` `list-scroll` with an in-place framebuffer scroll of the non-fade
body, followed by copying the exposed band, refreshing both fade bands, and
redrawing the fixed selection row/frame.

Raw traces:

- Before: `build/arcade-scroll-profiles/PR3-BEFORE-arcade-scroll.tsv`
- After: `build/arcade-scroll-profiles/PR3-AFTER-arcade-scroll.tsv`

Full/update frames before:

- `update=full` frames: 108
- `wall_us` p50/p95: 18,943 / 19,426
- `overlay_present_us` p50/p95: 2,390 / 2,898
- `rows` p50: 392

Scroll/update frames after:

- `update=scroll:-48` frames: 108
- `wall_us` p50/p95: 20,860 / 20,914
- `overlay_present_us` p50/p95: 4,313 / 4,362
- `rows` p50: 440

Reason:

The MiSTer write-combined framebuffer is a poor source for in-place scrolling.
The read/copy-within path roughly doubled overlay present cost, so this made the
scrolling frame miss budget worse. Future incremental approaches should avoid
reading from `/dev/fb0`; use normal-RAM composition or a smaller final write set
instead.
