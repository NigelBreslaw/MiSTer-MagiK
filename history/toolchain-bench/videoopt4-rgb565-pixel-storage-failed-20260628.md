# VideoOpt4 Rgb565Pixel Storage Attempt - 2026-06-28

Attempted optimization: store decoded video frames as `Rgb565Pixel` instead of `u16`, then pass the frame buffer directly to `present_rect_565_strided` without the presentation-side reinterpret cast.

Benchmark corpus: 320x240 half-res Neo Geo snaps from `build/video-snaps-neogeo-halfres`.

Results:

| label | fb-present avg | rows avg | fps | process CPU | visual/timing/capture |
| --- | ---: | ---: | ---: | ---: | --- |
| `VIDEOOPT4-BEFORE-20260628` | 274us | 239 | 59 | 38% | yes/yes/yes |
| `VIDEOOPT4-AFTER-20260628` | 273us | 239 | 59 | 38% | yes/yes/yes |

Detailed profile:

| metric | before | after |
| --- | ---: | ---: |
| final process CPU | 39.2% | 39.4% |
| video-scale avg | 1139us | 1152us |
| video-recv avg | 22us | 23us |
| audio-write avg | 255us | 262us |
| slow frames >= 16.667ms | 33.72% | 25.11% |

Diagnosis: the previous `u16` to `Rgb565Pixel` slice conversion was a zero-copy cast outside the measured hot path. Typing the decoded frame as `Rgb565Pixel` did not reduce CPU time per frame; the 1us `fb-present` change is within run noise, while final process CPU and conversion timing moved slightly worse.

Outcome: failed for the requested CPU-time-per-frame goal. The code change was not kept.
