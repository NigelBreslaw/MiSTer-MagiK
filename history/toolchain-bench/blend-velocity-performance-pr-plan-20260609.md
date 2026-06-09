# Blend Velocity Performance PR Plan - 2026-06-09

## Baseline

Standalone scene: `ui blend_velocity`.

Primary fair benchmark:

```bash
scripts/profile-blend-velocity.sh 15 BLENDVEL-TEXT real-text --deploy-fast
```

`real-text` uses cached arcade-style title rows with the same Press Start 2P
font/background path as the arcade list, while preserving the synthetic
6 px/frame velocity-scroll motion and split phase timings.

Initial MiSTer data:

| Variant | surface p50 | fade blend p50 | fade copy p50 | body copy p50 | wall p50 |
|---------|-------------|----------------|---------------|---------------|----------|
| `baseline` | 50 us | 1419 us | 226 us | 737 us | 16493 us |
| `real-text` | 48 us | 1390 us | 209 us | 703 us | 16504 us |
| `copy-only` | 51 us | 1 us | 221 us | 676 us | 16504 us |
| `no-fade` | 50 us | 1 us | 0 us | 885 us | 16509 us |

Conclusion: the procedural baseline is fair for raw fade blending. `real-text`
is close enough to use as the acceptance benchmark for production-like text
content. The main target is `fade_blend_us`, currently about 1.39-1.42 ms/frame.

Design constraint:

- Fade toward the arcade/list background color, not an assumed pure black.
- Current code uses `ARCADE_LIST_FADE_COLOR` (`0x00120d1a`), a dark background
  tone, so optimizations should keep supporting an arbitrary constant fade color.
- A black-only special case is acceptable only as a later optional path if the
  visual design actually chooses black and the benchmark proves it matters.

## PR 1: Make Real-Text Blend Velocity First-Class

Status: implemented in the benchmark scene.

Keep this as the benchmark baseline before any optimization PR. Use `real-text`
for acceptance and optionally `baseline` for lower-noise synthetic comparisons.

Gate:

- `real-text fade_blend_us` and `baseline fade_blend_us` remain within about 10%.
- scene remains runnable through `scripts/profile-blend-velocity.sh`.

## PR 2: Fix or Prove NEON Blend Path Usage

Hypothesis:

`blend_row_towards_neon` may not be compiled in current armv7 builds because the
cfg requires `target_feature = "neon"`, while the build only sets
`target-cpu=cortex-a9`. If the scalar path is active, enabling or guarding a real
NEON path should materially reduce `fade_blend_us`.

Work:

- Confirm which branch is compiled on MiSTer.
- If scalar is active, adjust RUSTFLAGS or cfg to enable the existing NEON path
  safely for Cortex-A9.
- Add a tiny runtime log or unit-level cfg guard so future builds do not silently
  lose NEON.

Gate:

- `real-text fade_blend_us` p50 improves by at least 20%.
- No regression in `wall_us` p95 or visual fade correctness.
- Run `scripts/profile-blend-velocity.sh 15 <label> real-text --deploy-fast`.

Drop if:

- NEON was already active, or enabling it does not clearly improve blend time.

## PR 3: Precompute Per-Row Fade Constants

Hypothesis:

The current blend path recomputes alpha-derived constants per row/frame. Moving
per-alpha constants into a small table may reduce scalar overhead and simplify
future specialized paths.

Work:

- Precompute `alpha`, `inv`, and color products for the 48 top and 48 bottom
  fade rows.
- Thread those constants into `blend_row_towards`.
- Preserve support for a non-black constant fade color.
- Keep the output byte-for-byte equivalent if practical.

Gate:

- `real-text fade_blend_us` p50 improves by at least 5%.
- No readability cost that outweighs a small win.

Drop if:

- Improvement is lost in noise or worsens the NEON path.

## PR 4: Cache Fully Blended Fade Rows For Stable Source Rows

Hypothesis:

During 6 px/frame velocity scroll, most fade rows are shifted versions of
recently blended source rows. A small ring cache keyed by source surface row and
fade alpha could avoid re-blending many rows.

Work:

- Cache blended fade rows in normal RAM.
- Invalidate when row content changes or when source row/alpha changes.
- Keep copy behavior unchanged.

Gate:

- `real-text fade_blend_us` p50 improves by at least 25%.
- `surface_us` and memory do not grow enough to erase the win.
- Confirm with `arcade_page held-scroll` after scene success.

Drop if:

- Cache lookup/invalidation is complex or stale pixels appear.

## PR 5: Split Fade Into Background Fill Plus Text Overlay

Hypothesis:

Most list pixels are background. Instead of blending every pixel toward black,
precompute or fill faded background-color bands and blend/copy only text/border
pixels on top.

Work:

- Prototype in `blend_velocity real-text` first.
- Represent row text/border mask or sparse spans.
- Use the configured/list background fade target, not a hardcoded black target.
- Preserve current visual look.

Gate:

- `real-text fade_blend_us + fade_copy_us` p50 improves by at least 30%.
- Visual output matches the current fade closely enough on HDMI.

Drop if:

- The mask/span machinery is too complex for the win.

## PR 6: Reduce Fade Height Or Adaptive Fade Region

Hypothesis:

Fade cost is proportional to rows. If design can tolerate a shorter fade, this
is the simplest performance lever.

Work:

- Test 48, 32, 24, and 16 px fade heights in `blend_velocity`.
- Compare visual appearance on HDMI and raw timings.

Gate:

- A visually acceptable height reduces `fade_blend_us` in proportion to rows.
- User accepts the appearance.

Drop if:

- The arcade page looks noticeably worse.

## PR 7: Apply Winning Blend Optimization To Arcade Page

Only after a standalone scene win.

Work:

- Port the winning approach into `ArcadeListRenderer::copy_fade_to_display`.
- Run:
  - `scripts/profile-blend-velocity.sh 15 <label> real-text`
  - `arcade_page held-scroll` raw trace
  - CPU flamegraph if the raw trace still points at blending

Gate:

- `arcade_page held-scroll overlay_present_us` p50 improves materially.
- `wall_us` p95 stays inside the 16.7 ms frame budget.
- No visible fade artifacts on HDMI.
