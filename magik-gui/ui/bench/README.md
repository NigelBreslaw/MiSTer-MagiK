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
printf 'mister_magik_suspend\n' > /dev/MiSTer_cmd
kill -9 $(pidof mister-magik-fb) 2>/dev/null
/media/fat/mister-magik/mister-magik-fb ui video_playback 20
printf 'mister_magik_restart_launcher\n' > /dev/MiSTer_cmd
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
path active and avoids runtime scaling in the hot frame loop.
`scripts/reencode-video-snaps-cortex-a9.sh` writes validated `640x480` to
`320x240` Lanczos-half, Constrained Baseline H.264/AAC assets under
`build/video-snaps-neogeo-cortex-a9`, with per-file provenance and a manifest.
`scripts/sync-video-snaps.sh` validates that manifest, stages the files on the
MiSTer, verifies remote hashes, and then swaps the playlist folder atomically so
stale full-size MP4s do not remain live.

Production `--video` supports only the maintained presentation paths:
direct framebuffer blit, `MISTER_VIDEO_SCALE=source`, and
`MISTER_VIDEO_SCALE=2x` for the explicit 320x240-to-640x480 pixel-doubled path.
Removed lab comparisons include Slint-image upload, FFmpeg swscale conversion,
alternate conversion backends, and decoder threading.

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
| `MISTER_VIDEO_QUEUE_DEPTH=N` | Decode worker channel depth, default 2 |
| `MISTER_VIDEO_SCALE=source` | Native-size presentation, used for 320x240 assets displayed at 320x240 |
| `MISTER_VIDEO_SCALE=2x` | Pixel-doubled presentation for 320x240 assets displayed at 640x480 |
| `MISTER_VIDEO_PROFILE=summary\|full\|trace` | Video-specific alias for `MISTER_PROFILE` |

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
scripts/bench-toolchain.sh P2 --skip-build --replace-label --device
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
scripts/reencode-video-snaps-cortex-a9.sh SOURCE_DIR
scripts/sync-video-snaps.sh
scripts/bench-toolchain.sh VIDEO-SNAPS --video --scene video_playback --video-scale source --replace-label
```
