# Video bench scene (toolchain / device)

`video_playback` is the maintained Slint bench scene. It uses the detected
MiSTer framebuffer size at runtime and copies 1:1 in every verified mode,
including 1080p, 960x540, 720p, 640x480, and CRT/direct-video smoke modes.
The retired synthetic Slint scenes are archived under
`history/bench-scenes/2026-06-retired-slint-scenes/`.

**Before a manual run**, stop only the Rust UI process. Leave `MiSTer_MagiK`
running so the launcher keeps its OSD/input suppression state. If display
ownership looks confused, reboot through the supervised
`scripts/mister reboot-wait` path instead of killing Main.

```bash
kill -9 $(pidof mister-magik-fb) 2>/dev/null
/media/fat/mister-magik/mister-magik-fb ui video_playback 20
```

With a production `--video` build, run `video_playback`. It accepts a single
looping file through `MISTER_VIDEO_PATH` or a filename-sorted `.mp4` folder
playlist through `MISTER_VIDEO_DIR`. The folder default is
`/media/fat/mister-magik/video-snaps/neogeo`, with the legacy
`/media/fat/mister-magik/mslug3.mov` clip as a compatibility fallback when the
folder is absent.

For 60 fps Neo Geo video snaps on the MiSTer, use half-resolution source assets
(`320x240` for the original `640x480` snaps) and leave
`MISTER_VIDEO_SCALE=source`. This keeps the fast YUV420P-to-RGB565 conversion
path active and avoids runtime upscaling or FFmpeg swscale in the hot frame
loop. `scripts/sync-video-snaps.sh` defaults to
`build/video-snaps-neogeo-halfres` and mirrors that set to the device so stale
full-size MP4s do not remain in the playlist.

Production `--video` supports the fast path only: `direct-blit`, `source`,
`custom-neon`, and decoder thread type `none`. Build with `--video-lab` when
comparing Slint-image upload, FFmpeg swscale conversion, non-source scaling, or
decoder threading experiments.

Classic camera/sprite/text/raster/transition effect scenes are experiments, not
production benchmark scenes. Build them with `scripts/deploy-rust.sh
--experiments` and run them through `scripts/experiments/`; see
`docs/experiments/effects.md`.

Shared window: [`../mister_window.slint`](../mister_window.slint).

## Profiling (per-frame timings + CPU flamegraph)

On device:

```bash
MISTER_PROFILE=1 \
MISTER_PROFILE_FILE=/tmp/frames.tsv \
MISTER_TRACE_FILE=/tmp/frames.json \
MISTER_PPROF=1 \
MISTER_PPROF_OUT=/tmp/cpu.svg \
/media/fat/mister-magik/mister-magik-fb ui video_playback 30
```

| Env | Effect |
|-----|--------|
| `MISTER_PROFILE=1` | Per-frame stats + summary (p50/p95/p99, histogram, worst frames) |
| `MISTER_PROFILE=slow` | Also log each frame over 16.667 ms |
| `MISTER_PROFILE=trace` | Summary + Chrome/Perfetto trace at `/tmp/mister-frame-trace.json` |
| `MISTER_PROFILE_FILE=…` | Write per-frame TSV |
| `MISTER_TRACE_FILE=…` | Write Chrome/Perfetto trace JSON |
| `MISTER_PPROF=1` | CPU flamegraph via `pprof` (needs `build-arm.sh --profile`; **may get 0 samples on MiSTer** — use frame TSV if so) |
| `MISTER_VIDEO_RENDER_MODE=direct-blit` | Production fast path |
| `MISTER_VIDEO_RENDER_MODE=slint-image` | Lab-only comparison; requires `--video-lab` |
| `MISTER_VIDEO_QUEUE_DEPTH=N` | Decode worker channel depth, default 2 |
| `MISTER_VIDEO_SCALE=source` | Production fast path; keeps the custom RGB565 converter eligible |
| `MISTER_VIDEO_SCALE=fit-height\|fit-width\|native` | Lab-only scaling modes; require `--video-lab` |
| `MISTER_VIDEO_PROFILE=summary\|full\|trace` | Video-specific alias for `MISTER_PROFILE` |
| `MISTER_VIDEO_THREADS=N` | Lab-only FFmpeg decoder thread count where supported |
| `MISTER_VIDEO_THREAD_TYPE=none\|frame\|slice\|auto` | `none` is production; other modes require `--video-lab` |
| `MISTER_VIDEO_CONVERT=custom-neon` | Production ARM custom YUV420P-to-RGB565 converter |
| `MISTER_VIDEO_CONVERT=swscale-rgb565` | Lab-only FFmpeg swscale fallback; requires `--video-lab` |

Phase breakdown each frame: **prepare** (input/catalog/bridge work before Slint
timers) · **anim** (Slint timers) · **slint-render** (software renderer) ·
**custom-draw** (project-owned drawing such as arcade list layers) · **vsync**
(`FBIO_WAITFORVSYNC`) · **fb-present** (dirty rect/rows → fb0). `fb-present` is
also split into **cached-present** and **overlay-present** where applicable.
Video builds also add **video-decode**, **video-scale**, **video-recv**,
**video-image**, **video-blit**, **audio-decode**, **audio-resample**, and
**audio-write**, plus queue depth, audio buffer frames, underrun, codec, size,
and file metadata. **wall** = whole iteration.

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

Toolchain bench (automated TSV + PNG — kills `mister-magik-fb` + MiSTer before each scene):

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-toolchain.sh P2 --skip-build --replace-label --device
```

`history/toolchain-bench/results.tsv` keeps the legacy `visual_ok` column as
the combined pass bit and adds `timing_ok` plus `capture_ok` so good frame
timing is not hidden by an agent framebuffer capture-route failure. Display
metadata is appended to the `notes` field per row: `physical_mode`, `fb_size`,
`render_size`, `fb_scale`, `pixel_repetition`, `uio_fb`, and `ini_mode`. PNG
captures come from `scripts/mister agent framebuffer-capture OUT.png --json
OUT.json`; the agent response records dimensions, stride, bpp, raw bytes, PNG
bytes, and stage timings.

Sync the local Neo Geo MP4 snaps and run only the video scene:

```bash
scripts/sync-video-snaps.sh
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-toolchain.sh VIDEO-SNAPS --video --scene video_playback --video-render-mode direct-blit --video-scale source --video-convert custom-neon --video-thread-type none --replace-label
```
