# Blend Velocity Fade Height Sweep - 2026-06-09

Benchmark: `blend_velocity real-text`, 15 seconds, 6 px/frame, PR7 benchmark
fade-height control. Production arcade fade height was not changed.

| Label | Fade h | fade_blend_us p50/p95 | fade_copy_us p50/p95 | body_copy_us p50/p95 | wall_us p50/p95 |
|-------|--------|------------------------|----------------------|----------------------|-----------------|
| `BLEND-PR7-48` | 48 px | `633` / `653` | `212` / `248` | `729` / `765` | `16491` / `16547` |
| `BLEND-PR7-32` | 32 px | `444` / `476` | `142` / `166` | `799` / `854` | `16484` / `16555` |
| `BLEND-PR7-24` | 24 px | `322` / `353` | `103` / `128` | `816` / `873` | `16474` / `16552` |
| `BLEND-PR7-16` | 16 px | `216` / `241` | `65` / `84` | `835` / `902` | `16482` / `16552` |

Finding:

- Fade blend and fade copy scale roughly with fade row count.
- Overall wall time stays close to the vsync budget in this standalone scene.
- Reducing fade height is a strong design lever, but it needs HDMI visual review
  before changing `ArcadeListRenderer`.
