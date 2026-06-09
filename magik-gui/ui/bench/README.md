# Slint bench scenes (toolchain / device)

Benchmark scenes use the detected MiSTer framebuffer size at runtime. The Slint
surface is the framebuffer size and copies 1:1 in every verified mode, including
1080p, 960x540, 720p, 640x480, and CRT/direct-video smoke modes. The old
960x540-to-1080p pixel-doubling path has been removed.

**Before a manual run**, stop anything else that owns SPI/HDMI (required for 60 fps):

```bash
kill -9 $(pidof mister-magik-fb) 2>/dev/null
kill -9 $(pidof MiSTer) 2>/dev/null
/media/fat/mister-magik/mister-magik-fb ui full_motion 20
```

Scenes: `demo`, `full_motion`, `static_ui`, `local_motion`, `console_scroll`,
and `blend_velocity`. With a `--video` build, `video_playback` is also
available. It expects `/media/fat/mister-magik/mslug3.mov` by default: H.264
baseline video plus 48 kHz stereo `pcm_s16le` audio. Override with
`MISTER_VIDEO_PATH`.

Shared window: [`../mister_window.slint`](../mister_window.slint).

## Profiling (per-frame timings + CPU flamegraph)

On device:

```bash
MISTER_PROFILE=1 \
MISTER_PROFILE_FILE=/tmp/frames.tsv \
MISTER_TRACE_FILE=/tmp/frames.json \
MISTER_PPROF=1 \
MISTER_PPROF_OUT=/tmp/cpu.svg \
/media/fat/mister-magik/mister-magik-fb ui full_motion 30
```

| Env | Effect |
|-----|--------|
| `MISTER_PROFILE=1` | Per-frame stats + summary (p50/p95/p99, histogram, worst frames) |
| `MISTER_PROFILE=slow` | Also log each frame over 16.667 ms |
| `MISTER_PROFILE=trace` | Summary + Chrome/Perfetto trace at `/tmp/mister-frame-trace.json` |
| `MISTER_PROFILE_FILE=…` | Write per-frame TSV |
| `MISTER_TRACE_FILE=…` | Write Chrome/Perfetto trace JSON |
| `MISTER_PPROF=1` | CPU flamegraph via `pprof` (needs `build-arm.sh --profile`; **may get 0 samples on MiSTer** — use frame TSV if so) |

Phase breakdown each frame: **prepare** (input/catalog/bridge work before Slint
timers) · **anim** (Slint timers) · **slint-render** (software renderer) ·
**custom-draw** (project-owned drawing such as arcade list layers) · **vsync**
(`FBIO_WAITFORVSYNC`) · **fb-present** (dirty rect/rows → fb0). `fb-present` is
also split into **cached-present** and **overlay-present** where applicable.
**wall** = whole iteration.

Host-side profile reports (no MiSTer packages required):

```bash
scripts/frame-profile-chart.py /tmp/frames.tsv /tmp/frames.svg
scripts/frame-profile-histogram.py /tmp/frames.tsv
scripts/frame-profile-slow-frames.py /tmp/frames.tsv --limit 12
scripts/frame-profile-heatmap.py /tmp/frames.tsv /tmp/dirty-heatmap.svg
scripts/frame-profile-report.py /tmp/frames.tsv /tmp/profile-report.html --trace /tmp/frames.json
```

Use the frame TSV/reports first. Use the CPU flamegraph only when a phase is
clearly CPU-bound and function attribution is needed; the in-process profiler
uses SIGPROF/ITIMER_PROF sampling and should be smoke-tested before trusting a
scene SVG.

## Blend Velocity Scene

`blend_velocity` is a Rust-only scene for isolating arcade-list fade/blend work
under velocity-scroll-like conditions. It synthesizes the moving arcade list
surface at 6 px/frame by default and reports split timings for surface update,
fade blend, fade copy, body copy, selection copy, vsync, and wall time.

```bash
scripts/profile-blend-velocity.sh 15 BLENDVEL baseline --deploy-fast
scripts/profile-blend-velocity.sh 15 BLENDVEL-TEXT real-text --skip-build
scripts/profile-blend-velocity.sh 15 BLENDVEL-COPY copy-only --skip-build
scripts/profile-blend-velocity.sh 15 BLENDVEL-NOFADE no-fade --skip-build
```

Variants:

- `baseline`: blend the top/bottom fades, copy fades, copy body, copy selection.
- `real-text`: same fade/copy path, but the moving surface uses cached title rows
  rendered with the arcade list font/background path.
- `copy-only`: copy the fade rows without blending, isolating framebuffer writes.
- `no-fade`: copy the moving body plus selection frame only.

Useful env:

- `MISTER_BLEND_BENCH_TRACE=/tmp/blend.tsv` writes per-frame split timings.
- `MISTER_BLEND_BENCH_VARIANT=baseline|copy-only|no-fade` chooses the variant.
- `MISTER_BLEND_BENCH_PX_PER_FRAME=6` changes synthetic scroll velocity.

Toolchain bench (automated TSV + PNG — kills `mister-magik-fb` + MiSTer before each scene):

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-toolchain.sh P2 --skip-build --replace-label --device
```

`history/toolchain-bench/results.tsv` keeps the historical schema. New display
metadata is appended to the `notes` field per row: `physical_mode`, `fb_size`,
`render_size`, `fb_scale`, `pixel_repetition`, `uio_fb`, and `ini_mode`. PNG
capture dimensions are parsed from the runtime log.

Include the video scene and upload the local 320×224 H.264 + PCM benchmark clip:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-toolchain.sh VIDEO --video --replace-label
```
