# Shared framebuffer-scene extraction baselines

These RGB565 hashes were captured from clean `main` at `5b002fac` before
portable framebuffer-scene authority was introduced. Hashes use FNV-1a over
the little-endian RGB565 pixel words.

## Particle scenes

The focused Slint-free lab rendered the embedded production recipes at logical
time 5,000 ms and 960x540.

| Scene | RGB565 hash |
|---|---:|
| MagiK | `aa213d52dc7eeeef` |
| cabinet | `01938d06cd4378ff` |

## Production navigation transitions

The existing headless UI preview used fixture content, HDMI 960x540, 60 Hz,
and captured frame 18. These hashes intentionally include the production
source and destination snapshots as well as the transition rasterizer.

| Edge | Direction | Progress Q16 | RGB565 hash |
|---|---|---:|---:|
| home-consoles | forward | 30,947 | `4a83d4978f99ef38` |
| home-arcade | forward | 25,789 | `91027672e7f5a9b1` |
| consoles-system | forward | 25,789 | `62c16f8b5b5cfeaf` |
| home-consoles | reverse | 30,948 | `9c86b69cfdf1f735` |
| home-arcade | reverse | 25,790 | `c45c776502db2adc` |
| consoles-system | reverse | 25,790 | `f6c8a5390b79115b` |

The pre-extraction performance comparison boundary remains the foreground
production renderer P99 of 4.704 ms recorded by the supplied migration plan.
PMU counters, thread placement, process telemetry, presentation, and device
lifecycle remain host-owned and are not part of these visual hashes.
