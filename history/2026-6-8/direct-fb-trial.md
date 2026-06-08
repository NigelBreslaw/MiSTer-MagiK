# Direct-FB Slint render trial - 2026-06-08

This note records the `direct-fb` experiment and why it was rejected despite
good benchmark numbers. The short version: rendering Slint directly into the
live MiSTer framebuffer removed the cached-to-framebuffer copy and reduced CPU,
but HDMI showed visible flicker. The production UI stayed on cached RAM render
plus a strict post-vsync dirty copy.

## Goal

The cached UI path renders Slint into a reused cached `Vec<Pixel>`, then copies
the dirty rows or rect into `/dev/fb0` after vblank. After the FPGA-scale work,
the UI framebuffer was only 960x540, so the remaining copy was roughly 0.7 ms
for normal animated scenes.

The experiment asked whether we could remove that copy entirely:

- `cached`: current cached RAM render, then dirty copy into `/dev/fb0`.
- `direct-fb`: render Slint directly into the mapped `/dev/fb0` slice.
- `line-fb`: render by line and copy line ranges into `/dev/fb0`.

The trial was benchmark-only first, selected by `MISTER_UI_RENDER_MODE`, then
briefly promoted into the launcher to check real HDMI behavior.

## Benchmark result

Raw rows are in `history/toolchain-bench/results.tsv` under:

- `PIPELINE-CACHED-20260608`
- `PIPELINE-DIRECT-FB-20260608`
- `PIPELINE-LINE-FB-20260608`

Representative rows:

| Mode | Scene | Render | Copy | FPS | CPU mean | Visual PNG |
|---|---|---:|---:|---:|---:|---|
| `cached` | `demo` | 907 us | 698 us | 60 | 9% | yes |
| `direct-fb` | `demo` | 811 us | 0 us | 60 | 5% | yes |
| `cached` | `full_motion` | 920 us | 712 us | 60 | 10% | yes |
| `direct-fb` | `full_motion` | 809 us | 0 us | 60 | 5% | yes |
| `cached` | `local_motion` | 149 us | 21 us | 61 | 1% | yes |
| `direct-fb` | `local_motion` | 141 us | 0 us | 61 | 1% | yes |

The framebuffer PNG captures looked correct. On CPU and parsed frame timing,
`direct-fb` looked like the winner: it saved the copy, slightly reduced render
time on broad animated scenes, and roughly halved CPU for the demo/full-motion
benchmark scenes.

`line-fb` did not beat either cached or direct-fb, and it added complexity, so it
was not considered for production.

## HDMI failure

The apparent benchmark win did not hold up on the real display. When
`direct-fb` was run as the launcher path, the HDMI output visibly flickered.

Mitigations tried:

- force `vsync-first`, so rendering starts immediately after vblank
- keep the 960x540 FPGA-scaled framebuffer path
- reboot the MiSTer before visual confirmation
- rerun the Slint frontend after reboot

The flicker remained. Cached rendering with `vsync-first` looked stable again.

The likely root cause is that `/dev/fb0` is the live scan-out buffer. Direct
Slint rendering does not have a strict present boundary: the software renderer
can write many small spans into the live buffer while the FPGA is scanning that
same memory. Waiting for vblank first improves timing, but it is still not a
real backbuffer and cannot guarantee the entire dirty region is complete before
scan-out reaches it.

This matches the older framebuffer finding: `/dev/fb0` is the only
write-combined buffer we can access through the Linux framebuffer driver. The
other MiSTer framebuffer slots can be addressed through `/dev/mem`, but those
mappings are uncached device memory and are much slower. They are not a viable
Slint backbuffer on this hardware.

## Decision

Rejected for production:

- `MISTER_UI_RENDER_MODE=direct-fb`
- `line-fb`
- launcher default direct render into `/dev/fb0`

Kept:

- cached RAM Slint rendering
- `vsync-first` support
- the small 960x540 `/dev/fb0` plus FPGA scaler path
- dirty-row/dirty-rect copy after vblank

The accepted stable path is:

1. wait for vblank (`vsync-first`)
2. render Slint into cached RAM
3. copy the dirty rows/rect into the live write-combined `/dev/fb0`

That gives a hard render/copy boundary while avoiding direct writes into the
live scan-out buffer during Slint's draw pass.

## Follow-up direction

The way to revisit this is not another direct render into `/dev/fb0`. It would
need a real non-live, write-combined framebuffer that can be presented atomically
or close to atomically. Without that, the productive optimization work is on the
stable cached path:

- reduce dirty area
- keep `vsync-first`
- improve copy kernels only when the benchmark proves it
- use video-specific paths where Slint image rendering is the actual bottleneck

