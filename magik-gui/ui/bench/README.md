# Slint bench scenes (toolchain / device)

Benchmark scenes use the detected MiSTer framebuffer size at runtime. At the
legacy 1080p HDMI mode they still render a 960x540 surface and 2x upscale into
1920x1080. At lower framebuffer modes, such as 960x540, 720p, and 640x480, they
render 1:1 with no pixel-doubling path.

**Before a manual run**, stop anything else that owns SPI/HDMI (required for 60 fps):

```bash
kill -9 $(pidof mister-magik-fb) 2>/dev/null
kill -9 $(pidof MiSTer) 2>/dev/null
/media/fat/mister-magik/mister-magik-fb ui full_motion 20
```

Scenes: `demo`, `full_motion`, `static_ui`, `local_motion`, `console_scroll`.
With a `--video` build, `video_playback` is also available. It expects
`/media/fat/mister-magik/mslug3.mov` by default: H.264 baseline video plus
48 kHz stereo `pcm_s16le` audio. Override with `MISTER_VIDEO_PATH`.

Shared window: [`../mister_window.slint`](../mister_window.slint).

## Profiling (per-frame timings + CPU flamegraph)

On device:

```bash
MISTER_PROFILE=1 \
MISTER_PROFILE_FILE=/tmp/frames.tsv \
MISTER_PPROF=1 \
MISTER_PPROF_OUT=/tmp/cpu.svg \
/media/fat/mister-magik/mister-magik-fb ui full_motion 30
```

| Env | Effect |
|-----|--------|
| `MISTER_PROFILE=1` | Per-frame stats + summary (p50/p95/p99, histogram, worst frames) |
| `MISTER_PROFILE=slow` | Also log each frame over 16.667 ms |
| `MISTER_PROFILE_FILE=…` | Write per-frame TSV |
| `MISTER_PPROF=1` | CPU flamegraph via `pprof` (needs `build-arm.sh --profile`; **may get 0 samples on MiSTer** — use frame TSV if so) |

Phase breakdown each frame: **anim** (Slint timers) · **render** (software renderer) · **vsync** (`FBIO_WAITFORVSYNC`) · **copy** (dirty rect/rows → fb0, includes 2× upscale). **wall** = whole iteration.

Toolchain bench (automated TSV + PNG — kills `mister-magik-fb` + MiSTer before each scene):

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-toolchain.sh P2 --skip-build --replace-label --device
```

`history/toolchain-bench/results.tsv` keeps the historical schema. New display
metadata is appended to the `notes` field per row: `physical_mode`, `fb_size`,
`render_size`, `fb_scale`, `pixel_repetition`, `uio_fb`, `bench_render_scale`,
and `ini_mode`. PNG capture dimensions are parsed from the runtime log instead
of assuming 1920x1080.

Include the video scene and upload the local 320×224 H.264 + PCM benchmark clip:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-toolchain.sh VIDEO --video --replace-label
```
