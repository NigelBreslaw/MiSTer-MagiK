# Blend Velocity Arcade Confirmation - 2026-06-09

Production confirmation for the kept standalone win:

- Standalone winner: PR4 precomputed fade constants.
- Production path: `ArcadeListRenderer` uses the same precomputed constants for
  `copy_fade_to_target`.
- Scenario: `arcade_page held-scroll`, 15 seconds, raw frame trace.

| Label | overlay_present_us p50/p95 | fb_present_us p50/p95 | arcade_draw_us p50/p95 | wall_us p50/p95 |
|-------|-----------------------------|-----------------------|------------------------|-----------------|
| `VEL-HELD-20260609` | `2743` / `2912` | `2755` / `2922` | `61` / `481` | `16417` / `16605` |
| `BLEND-PR8-HELD` | `1649` / `1935` | `1667` / `1952` | `53` / `448` | `16426` / `16523` |

Result:

- `overlay_present_us` p50 improved by about `40%`.
- `overlay_present_us` p95 improved by about `34%`.
- `wall_us` p95 improved from `16605us` to `16523us`.
- Rows copied stayed the same (`384` p50, `392` p95), so this confirms the fade
  blend/present path got cheaper rather than the workload changing shape.

Artifacts:

- `build/arcade-scroll-profiles/BLEND-PR8-HELD-arcade-scroll.tsv`
- `build/arcade-scroll-profiles/BLEND-PR8-HELD-arcade-scroll.log`
