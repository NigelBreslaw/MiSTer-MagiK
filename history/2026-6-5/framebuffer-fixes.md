# Framebuffer fixes and benchmark results - 2026-06-05

This note records the framebuffer/display fixes made on 2026-06-05, why they
were made, and the MiSTer benchmark scores before and after. The goal was to
review `rust/src/fb.rs` for clarity and performance, fix the issues, and prove
the result on the MiSTer rather than trusting the code by inspection.

## Benchmark setup

Before label:

```bash
MISTER_BENCH_SETTLE_SECS=5 scripts/bench-toolchain.sh \
  FBREVIEW-BEFORE-20260605 --skip-build --device --replace-label --scene-secs 15
```

After label:

```bash
MISTER_BENCH_SETTLE_SECS=5 scripts/bench-toolchain.sh \
  FBREVIEW-AFTER-20260605 --skip-build --device --replace-label --scene-secs 15
```

Both runs used:

- MiSTer at `192.168.1.117`
- release-device ARM build
- render scale 1: Slint renders 960x540, framebuffer copy scales 2x to 1920x1080
- `MISTER_BENCH_SETTLE_SECS=5` to avoid the known contaminated 30fps state
- clean kill of `mister-magic-fb` and `MiSTer` before each scene

Raw rows are in `history/toolchain-bench/results.tsv`.

## Fixes

### 1. Cache the glyph alpha threshold

Problem:

`Pixel::blend()` called `std::env::var("MISTER_GLYPH_ALPHA_THRESHOLD")` for every
blended pixel. This is deep in Slint's software-renderer hot path, especially for
text and alpha masks.

Fix:

Cache the parsed threshold once with `OnceLock<u8>`.

Impact:

This was the big win. The `text_heavy` scene render time dropped from 118.363 ms
to 23.183 ms, and `list_scroll` render time dropped from 27.861 ms to 4.620 ms.

### 2. Remove the misleading framebuffer-to-framebuffer scroll path

Problem:

A prototype `scroll_rect_y()` copied pixels within `/dev/fb0`. It looked like a
classic console trick, but on MiSTer this reads from the write-combined
framebuffer mapping. That is slow. A test version ran at about 30fps with
`fb-scroll` around 29 ms.

Fix:

Remove the unused `scroll_rect_y()` path from `Display`. The working console
scroll demo keeps a logical shadow surface in cached RAM, scrolls that, redraws
the exposed strip, then writes to `/dev/fb0`.

Impact:

Avoids preserving a false "fast path" in the display API. The measured fast
console path is now benchmarked as `console_scroll`.

### 3. Validate framebuffer layout

Problem:

`Display::open()` assumed the framebuffer was 1920x1080x32 with the expected
channel order and stride. That is fine for the current hardcoded path but brittle
for future live-mode/CRT work.

Fix:

Query and validate:

- visible width
- virtual/visible height
- 32 bits per pixel
- RGB channel offsets `r16 g8 b0`
- fixed framebuffer stride via `FBIOGET_FSCREENINFO`

Important gotcha:

The correct Linux ioctl number for `FBIOGET_FSCREENINFO` is `0x4602`. A first
attempt used `0x4601`, which returned nonsense (`line_length=8`) and made the UI
fail to open `/dev/fb0`. The final code uses `0x4602`, matching Slint's own
Linux framebuffer backend.

### 4. Report vsync ioctl failure once

Problem:

`wait_vsync()` ignored `FBIO_WAITFORVSYNC` failures. If the ioctl failed, the UI
could silently run unpaced and make benchmark data meaningless.

Fix:

Log the first `FBIO_WAITFORVSYNC` failure with `AtomicBool` so it is visible but
does not spam every frame.

Impact:

No benchmark change expected. This is correctness/diagnostics.

### 5. Add a 2x copy specialization without framebuffer reads

Problem:

The generic scaled-copy loop is clear, but scale 2 is the dominant current path.
A first specialization accidentally copied the second scaled row by reading back
from `/dev/fb0`, which reintroduced the slow framebuffer-read problem. That bad
intermediate run showed copy times around 7 ms for normal scenes and up to 30 ms
for full-width text-heavy scenes.

Fix:

The final 2x path writes both scaled rows directly from the cached source row.
It never reads from `/dev/fb0`.

Impact:

Copy times stayed healthy for partial-dirty scenes and improved a little for
near-full-screen scenes:

- `solid_fill` copy: 10.336 ms -> 9.152 ms
- `text_heavy` copy: 10.744 ms -> 9.606 ms
- normal partial scenes remained about the same

## Benchmark results

Columns:

- `render`: Slint render time, except for `console_scroll`
- `copy`: cached RAM -> framebuffer copy time, except for `console_scroll`
- `cpu`: mean CPU percent reported by the benchmark script
- `fps`: average parsed scene fps

For `console_scroll`, the TSV stores:

- `render_us` = RAM shadow-surface scroll
- `vsync_us` = exposed-strip redraw
- `copy_us` = framebuffer copy

| Scene | FPS before | FPS after | Render before | Render after | Copy before | Copy after | CPU before | CPU after |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| `demo` | 60 | 60 | 1.925 ms | 0.890 ms | 2.299 ms | 2.309 ms | 24% | 18% |
| `full_motion` | 60 | 60 | 1.924 ms | 0.851 ms | 2.309 ms | 2.278 ms | 24% | 18% |
| `static_ui` | 61 | 61 | 0.001 ms | 0.001 ms | 0.001 ms | 0.001 ms | 0% | 0% |
| `local_motion` | 61 | 61 | 0.256 ms | 0.139 ms | 0.057 ms | 0.052 ms | 2% | 1% |
| `text_heavy` | 8 | 28 | 118.363 ms | 23.183 ms | 10.744 ms | 9.606 ms | 90% | 86% |
| `solid_fill` | 58 | 58 | 4.841 ms | 4.854 ms | 10.336 ms | 9.152 ms | 86% | 79% |
| `list_scroll` | 28 | 60 | 27.861 ms | 4.620 ms | 4.038 ms | 4.413 ms | 83% | 52% |
| `console_scroll` | n/a | 60 | n/a | 2.010 ms ram-scroll | n/a | 6.204 ms fb-copy | n/a | 46% |

## Takeaways

- Do not read animated pixels back from `/dev/fb0`. The MiSTer framebuffer map is
  good for write-combined writes, not for fast read/modify animation.
- Keep hot-path configuration out of `TargetPixel::blend()`. Even a tiny-looking
  environment lookup can dominate text-heavy scenes.
- The ordinary Slint `list_scroll` scene now reaches 60fps after the alpha
  threshold cache, but the console-style shadow-surface path is still useful:
  it gives deterministic 60fps scrolling with Press Start 2P text and bounded
  redraw work.
- `text_heavy` is much better but still not a 60fps workload. It remains a stress
  scene, not a production target.

## Remaining notes

- `cargo fmt` could not be run on the host because `rustfmt` is not installed for
  `stable-aarch64-apple-darwin`. The ARM device build passed.
- The benchmark script now includes `console_scroll` in the default scene list
  and documents its metric column mapping in the TSV notes.
