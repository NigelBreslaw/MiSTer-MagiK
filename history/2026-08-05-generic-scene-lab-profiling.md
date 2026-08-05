<!--
Copyright (C) 2026 Nigel Breslaw
SPDX-License-Identifier: GPL-3.0-or-later
-->

# Generic timed scene-lab profiling qualification

Date: 2026-08-05

Functional commit: `494f2c096e40e32254a9b3a8f498d3b1425b8de9`

The functional series added bounded duration and warm-up handling, scene-neutral
confirmation evidence, generic 99 Hz CPU sampling, per-second RSS sampling,
scene-scoped retrieval, and two-pass assessment to the attended MiSTer scene
lab. A non-publishing pre-push run selected 29 checks and passed.

The MiSTer display remained `video_mode=1920,1200,60` throughout. The detected
plan was a 960×600 RGB565 render/framebuffer, 1920×1200 scan/output, HDMI route.
The final typed status reported `LauncherActive`, the Dev Main and launcher
processes healthy, and the same display mode.

## Screenshot screensaver assessment

Command:

```text
scripts/agent device scene-lab --scene screenshot-screensaver --seconds 90 --assess --attended
```

Evidence: `build/scene-lab/screenshot-screensaver/1785937896/`

Binary SHA-256: `2ded2fa4f6599e2fa1abe9f562cb8dabff219725665c24c25083e8be1cd17c7f`

Installed pack: 24,326,278 bytes,
SHA-256 `387728d3d0cf2aa2f2e5b8d56ecc72f63e8d4afc46ac97476dbd13d3ed360ee3`

Seed: `0x4d6167694b54696c`; sampling profile: `hdmi`.

| Metric | Unprofiled cadence | 99 Hz sampled |
| --- | ---: | ---: |
| Confirmed frames | 5,397 | 5,378 |
| Unique latch flips | 5,396 | 5,377 |
| Unique presentation FPS | 59.953 | 59.950 |
| Repeated refreshes | 0 | 0 |
| Sequence failures | 0 | 0 |
| Latch drops | 0 | 0 |
| Completion failures | 0 | 0 |
| Long completion intervals | 0 | 0 |
| Render mean / P99 | 6.380 / 8.165 ms | 6.439 / 8.273 ms |
| Transfer mean / P99 | 1.510 / 1.947 ms | 1.567 / 2.115 ms |
| Post mean / P99 | 0.098 / 0.168 ms | 0.097 / 0.172 ms |
| Settle mean / P99 | 5.818 / 8.135 ms | 6.581 / 10.695 ms |
| Frame-to-confirm mean / P99 | 16.666 / 16.838 ms | 16.669 / 16.842 ms |
| Process CPU | 78.97% | 81.74% |
| RSS mean / maximum | 28,192 / 36,488 KiB | 79,669 / 87,940 KiB |

The sampled profile contains 7,300 hits across 681 stack groups, with complete
profile metadata, folded stacks, and flamegraph. Only the unprofiled pass is the
cadence authority; it qualified with no physical frame drops.

## Cross-scene checks

The five-second `navigation-transition` measurement retained evidence at
`build/scene-lab/navigation-transition/1785938131/`. It reported 258 confirmed
frames, 51.701 unique FPS, 41 repeated refreshes/long intervals, and zero
sequence failures or latch drops. This intentionally demonstrates that the
generic recorder detects physical skips independently of latch-drop counters
and retains evidence on qualification failure.

The five-second card-flip assessment retained evidence at
`build/scene-lab/card-flip/1785938247/`. Its authoritative pass reported 301
confirmations, 59.949 FPS, and zero repeated refreshes, sequence failures,
latch drops, completion failures, or long intervals. Its sampled pass collected
171 hits across 47 stack groups. Card geometry (287×420), progress, face,
direction, dirty rectangles, and byte counts remained present in the raw frame
evidence. Unprofiled render mean/P99 was 3.886/6.228 ms and sampled was
4.016/6.400 ms.

An initial ten-second card sampled pass terminated with SIGSEGV before profile
artifacts were finalized. Typed cleanup restored the launcher and a subsequent
health check confirmed the unchanged display. One shorter bounded assessment
then completed successfully; the incident is recorded rather than hidden.
