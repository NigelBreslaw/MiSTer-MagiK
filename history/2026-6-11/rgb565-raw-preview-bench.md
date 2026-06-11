# RGB565 and raw preview blitter bench

Date: 2026-06-11

Scope: arcade held-scroll benchmark on MiSTer, real launcher and standalone
`preview_scroll_bench`, with deterministic visual captures at selected indices
0, 7, 14, and 21.

Commands:

```bash
scripts/profile-preview-scroll.sh 20 held-scroll RGB565-BEFORE-8888-20260611 --fb-format 8888 --deploy-fast --visual-captures 4
scripts/profile-preview-scroll.sh 20 held-scroll RGB565-AFTER-565-20260611 --fb-format 565 --deploy-fast --visual-captures 4
scripts/profile-preview-scroll.sh 20 held-scroll RGB565-RAW-565-20260611 --fb-format 565 --preview-blitter raw --deploy-fast --visual-captures 4
scripts/profile-preview-scroll.sh 30 turbo-hold RGB565-BEFORE-8888-TURBO-20260611 --fb-format 8888 --preview-blitter slint --skip-build --visual-captures 0
scripts/profile-preview-scroll.sh 30 turbo-hold RGB565-RAW-565-TURBO-20260611 --fb-format 565 --preview-blitter raw --skip-build --visual-captures 0
scripts/profile-preview-scroll.sh 30 screenshot-stress RGB565-BEFORE-8888-STRESS-20260611 --fb-format 8888 --preview-blitter slint --skip-build --visual-captures 0
scripts/profile-preview-scroll.sh 30 screenshot-stress RGB565-RAW-565-STRESS-20260611 --fb-format 565 --preview-blitter raw --skip-build --visual-captures 0
```

Real launcher results:

| Run | p95 wall | >16.7ms | >20ms | broad Slint p50/p95 | broad custom p50/p95 | broad present p50/p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 8888 Slint image | 17.295 ms | 160 | 3 | 10.477 / 12.135 ms | 0.280 / 0.477 ms | 2.717 / 2.958 ms |
| 565 Slint image | 16.996 ms | 155 | 2 | 9.541 / 11.225 ms | 0.259 / 0.534 ms | 3.252 / 3.449 ms |
| 565 raw blitter | 16.964 ms | 155 | 1 | 0.273 / 0.611 ms | 3.118 / 4.091 ms | 3.105 / 3.440 ms |

Additional real launcher scenario checks:

| Scenario | Run | p95 wall | >16.7ms | >20ms | Slint p95 | custom p95 | present p95 | preview applies |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| turbo-hold | 8888 Slint image | 17.365 ms | 439 | 3 | 11.733 ms | 0.397 ms | 2.856 ms | 430 |
| turbo-hold | 565 raw blitter | 17.007 ms | 441 | 1 | 0.487 ms | 3.830 ms | 3.341 ms | 433 |
| screenshot-stress | 8888 Slint image | 17.267 ms | 432 | 3 | 11.578 ms | 0.004 ms | 0.803 ms | 432 |
| screenshot-stress | 565 raw blitter | 17.008 ms | 428 | 1 | 0.389 ms | 3.406 ms | 0.509 ms | 432 |

Findings:

- RGB565 alone reduced broad screenshot-frame Slint render p50 by about 9% in
  the real launcher, so it did not meet the 15% acceptance threshold on its own.
- Raw blitter mode removed the Slint image-render spike: broad screenshot-frame
  Slint render p50 dropped from 10.477 ms to 0.273 ms versus the 8888 baseline.
- The work moved into `custom_draw_us` as intended, with broad-frame raw blit
  p50 about 3.118 ms.
- The deterministic visual captures matched the same selected games and looked
  acceptable in framebuffer PNGs. HDMI still needs a human pass for final
  production default.
- Turbo-hold and screenshot-stress both kept applying previews during motion.
  Raw mode reduced Slint p95 by an order of magnitude in both scenarios and cut
  >20 ms frames from 3 to 1.
- After this pass, normal launcher defaults were changed to RGB565 framebuffer
  plus raw preview blitter. Use `MISTER_FB_FORMAT=8888` and
  `MISTER_PREVIEW_BLITTER=slint` to force the old path.

Artifacts:

- `build/preview-scroll-profiles/RGB565-BEFORE-8888-20260611-*`
- `build/preview-scroll-profiles/RGB565-AFTER-565-20260611-*`
- `build/preview-scroll-profiles/RGB565-RAW-565-20260611-*`
- `build/rgb565-smoke-scaled-snapshot/fb0.png`
