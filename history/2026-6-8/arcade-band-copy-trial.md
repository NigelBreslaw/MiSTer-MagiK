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
