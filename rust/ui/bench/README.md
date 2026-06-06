# Slint bench scenes (toolchain / device)

All scenes render at **960×540**; Rust upscales 2× to 1920×1080 HDMI.

**Before a manual run**, stop anything else that owns SPI/HDMI (required for 60 fps):

```bash
kill -9 $(pidof mister-magic-fb) 2>/dev/null
kill -9 $(pidof MiSTer) 2>/dev/null
/media/fat/mister-magic/mister-magic-fb ui full_motion 20
```

Or use **`scripts/bench-diagnose.sh visible …`** (streams progress, no timeout).

Scenes: `demo`, `full_motion`, `static_ui`, `local_motion`, `text_heavy`, `solid_fill`, `list_scroll`, `console_scroll`, `dirty_band`.
With a `--video` build, `video_playback` is also available. It expects
`/media/fat/mister-magic/mslug3.mov` by default: H.264 baseline video plus
48 kHz stereo `pcm_s16le` audio. Override with `MISTER_VIDEO_PATH`.

### dirty_band — copy budget sweep

Solid-color band scrolling vertically. Set band height (% of 540 logical rows) via env:

```bash
MISTER_DIRTY_BAND_PCT=50 /media/fat/mister-magic/mister-magic-fb ui dirty_band 15
```

Sweep 10–100% and print the ~60 fps cutoff:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench_dirty_band.sh
```

Shared window: [`../mister_window.slint`](../mister_window.slint).

## Profiling (per-frame timings + CPU flamegraph)

Build a symbols + pprof binary, deploy, run a scene, pull artifacts:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/profile-scene.sh full_motion 30
```

On device manually:

```bash
MISTER_PROFILE=1 \
MISTER_PROFILE_FILE=/tmp/frames.tsv \
MISTER_PPROF=1 \
MISTER_PPROF_OUT=/tmp/cpu.svg \
/media/fat/mister-magic/mister-magic-fb ui full_motion 30
```

| Env | Effect |
|-----|--------|
| `MISTER_PROFILE=1` | Per-frame stats + summary (p50/p95/p99, histogram, worst frames) |
| `MISTER_PROFILE=slow` | Also log each frame over 16.667 ms |
| `MISTER_PROFILE_FILE=…` | Write per-frame TSV |
| `MISTER_PPROF=1` | CPU flamegraph via `pprof` (needs `build-arm.sh --profile`; **may get 0 samples on MiSTer** — use frame TSV if so) |

Phase breakdown each frame: **anim** (Slint timers) · **render** (software renderer) · **vsync** (`FBIO_WAITFORVSYNC`) · **copy** (dirty rect/rows → fb0, includes 2× upscale). **wall** = whole iteration.

Host-side TSV rollup: `python3 scripts/analyze-frame-profile.py history/toolchain-bench/profile-*/mister-frame-*.tsv`

Toolchain bench (automated TSV + PNG — kills `mister-magic-fb` + MiSTer before each scene):

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-toolchain.sh P2 --skip-build --replace-label --device
```

Include the video scene and upload the local 320×224 H.264 + PCM benchmark clip:

```bash
MISTER_IP=192.168.1.117 MISTER_PASS=1 scripts/bench-toolchain.sh VIDEO --video --replace-label
```
