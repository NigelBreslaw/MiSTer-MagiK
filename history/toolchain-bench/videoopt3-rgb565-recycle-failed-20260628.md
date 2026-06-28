# VideoOpt3 RGB565 Recycle Attempt - 2026-06-28

Attempted optimization: recycle decoded RGB565 `Vec<u16>` frame buffers through the existing video worker recycle channel, beside the already recycled audio buffer.

Benchmark corpus: 320x240 half-res Neo Geo snaps from `build/video-snaps-neogeo-halfres`.

Results:

| label | fb-present avg | rows avg | fps | process CPU | visual/timing/capture |
| --- | ---: | ---: | ---: | ---: | --- |
| `VIDEOOPT3-BEFORE-20260628` | 274us | 239 | 59 | 38% | yes/yes/yes |
| `VIDEOOPT3-AFTER-20260628` | 275us | 239 | 59 | 38% | yes/yes/yes |

Detailed profile:

| metric | before | after |
| --- | ---: | ---: |
| final process CPU | 39.5% | 39.4% |
| video-scale avg | 1141us | 1143us |
| video-recv avg | 22us | 21us |
| audio-write avg | 262us | 264us |
| slow frames >= 16.667ms | 22.60% | 34.00% |

Diagnosis: at 320x240, one RGB565 frame is about 150 KiB. The allocator work from creating that vector each decoded frame is not a visible part of CPU time per frame after options 1 and 2 removed the larger present/blit costs. The measured hot path remains NEON I420-to-RGB565 conversion plus H.264/AAC processing, not RGB565 buffer allocation.

Memory impact was not proven by this benchmark. `results.tsv` recorded `rss_kb=0` for both rows, so the current toolchain RSS sampling is not reliable for this scene. A future memory-focused retry should sample `VmRSS`/`VmHWM` from `/proc/$pid/status` while the scene is running.

Outcome: failed for the requested CPU-time-per-frame goal. The code change was not kept.
