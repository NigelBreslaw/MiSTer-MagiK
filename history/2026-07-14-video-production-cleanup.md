# Video Production Cleanup

Date: 2026-07-14
Branch: `nigel/video-production-cleanup`

## Outcome

The video system was reduced to the maintained production shape:

- H.264/AAC decode is handled by the minimal Cortex-A9 FFmpeg build.
- Project-owned frame conversion is plain Rust RGB565 code.
- `MISTER_VIDEO_SCALE=source` displays decoded frames at their source size.
- `MISTER_VIDEO_SCALE=2x` pixel-doubles already-half-size assets when a
  640x480 presentation is required.
- Frame presentation uses the direct framebuffer blit path.
- The decode worker recycles RGB565 frame buffers.
- Audio filling no longer does hidden video packet work.

Removed production complexity:

- Rust ARM NEON video conversion/scaler branches.
- C scalar/C NEON video conversion backends.
- FFmpeg swscale comparison builds.
- Slint-image video upload.
- Decoder-thread comparison knobs.
- Lab-only scale modes and video-lab feature/build surface.

## Current NEON contract

There is no project-owned NEON video kernel left in the production code. The ARM
build still targets Cortex-A9 through Rust `target-cpu=cortex-a9`, but it no
longer relies on a global Rust `+neon` flag for video. NEON acceleration is
expected inside FFmpeg's ARM H.264 decoder code.

`magik-gui/scripts/build-minimal-ffmpeg.sh` now verifies the FFmpeg generated
configuration and archive:

- `ARCH_ARM=1`
- `HAVE_NEON=1`
- `CONFIG_RUNTIME_CPUDETECT=1`
- configure/build logs contain `cortex-a9` and `-mfpu=neon-vfpv3`
- `libavcodec.a` contains H.264 NEON objects

## Asset policy

The canonical asset path is:

```bash
scripts/reencode-video-snaps-cortex-a9.sh SOURCE_DIR
scripts/sync-video-snaps.sh
```

The encoder script creates ignored assets under
`build/video-snaps-neogeo-cortex-a9` by default. Each source clip must be
`640x480`, is halved to `320x240` with Lanczos, encoded as H.264 Constrained
Baseline, and keeps AAC audio copied from the source.

Validation requires:

- H.264 Constrained Baseline or Baseline
- no CABAC, B-frames, or multi-reference frames
- fixed GOP
- `yuv420p`
- copied AAC audio
- matching half geometry and source frame rate
- SSIM at least `0.995`
- luma PSNR at least `45 dB`
- per-file provenance and a manifest with SHA-256 hashes

The sync script validates the manifest locally, uploads into a remote staging
directory, verifies hashes on the MiSTer, and only then swaps the live playlist
folder. This avoids leaving mixed old/new video sets live after a failed copy.

## Rejected or deferred approaches

- Native 640x480 playback, even with Constrained Baseline H.264, did not become
  the winning Cortex-A9 path. The RGB565 conversion cost at full resolution was
  still too high for comfortable 60 fps.
- Nearest-neighbour downscale was visually worse than Lanczos for the tested
  low-resolution arcade content. Runtime 2x presentation remains nearest
  neighbour because it is a literal pixel-doubling path.
- Direct hidden-slot fused presentation remains deferred. It could remove the
  final cached-RAM to scanout-slot copy, but it needs a careful lease/state
  machine so the worker never writes an active or pending scanout slot.
- RGB565 buffer recycling is retained because it is simple, production-safe, and
  removes allocator/cache churn without adding a second rendering path.
