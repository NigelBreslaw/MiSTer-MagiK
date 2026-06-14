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
  plus raw previews. Use `MISTER_FB_FORMAT=8888` only for diagnostic
  framebuffer/color-route A/B runs.
- Keep 8888 support for smoke tests until the boot/fallback path is fully
  settled.

Artifacts:

- `build/preview-scroll-profiles/RGB565-BEFORE-8888-20260611-*`
- `build/preview-scroll-profiles/RGB565-AFTER-565-20260611-*`
- `build/preview-scroll-profiles/RGB565-RAW-565-20260611-*`
- `build/rgb565-smoke-scaled-snapshot/fb0.png`

## 8888 diagnostic policy check - 2026-06-14

Current held-scroll, raw565 previews, same deployed build after the launcher
catalog projection:

| Format | Frames | Avg wall | P95 wall | Slow >16.7 ms | Slow >20 ms |
|---|---:|---:|---:|---:|---:|
| 565 | 590 | 16.642 ms | 17.082 ms | 80 | 1 |
| 8888 | 530 | 18.552 ms | 28.703 ms | 202 | 150 |

The 8888 override is retained for diagnostics, but production/default arcade
profiling should use RGB565 unless a later architecture removes the extra
presentation cost.

## Native raw RGB565 preview cache follow-up

Date: 2026-06-11

Change: added `PreviewStorageFormat::RawRgb565`, storing resized
little-endian RGB565 rows with 16-byte-aligned stride under
`screenshot-magik/raw565-nearest-320x320/*.rgb565`. The default preview storage
is now raw565 when the framebuffer is RGB565 and the raw blitter is enabled;
`MISTER_PREVIEW_FORMAT` still overrides it.

Cache build on MiSTer:

```bash
mister-magik-fb preview-cache build --format raw-rgb565 --filter nearest --max 320x320 --root /media/fat/_Arcade
```

Result: 904 files converted, 0 failures, 146,841,760 output bytes,
30.5 s elapsed. Average build-time costs were read 2.337 ms, PNG decode
3.186 ms, resize 9.197 ms, encode/write 18.732 ms.

Real launcher held-scroll, 30 s, RGB565 framebuffer and raw blitter:

| Run | frames | avg wall | p95 wall | >16.7ms | >20ms | custom avg | custom p50 | custom p95 | Slint avg | Slint p95 | decoded | read avg | decode avg | resize avg | load total avg |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Pre-change raw-rgb deployed baseline | 1786 | 16.561 ms | 17.242 ms | 229 | 1 | 0.493 ms | 0.052 ms | 3.567 ms | 0.187 ms | 0.330 ms | 220 | 11.447 ms | 2.392 ms | 4.264 ms | 18.115 ms |
| Same-build raw-rgb | 1801 | 16.412 ms | 17.002 ms | 215 | 1 | 0.551 ms | 0.054 ms | 4.304 ms | 0.070 ms | 0.366 ms | 220 | 0.594 ms | 2.590 ms | 4.153 ms | 7.351 ms |
| After raw-rgb565 | 1802 | 16.439 ms | 16.950 ms | 218 | 1 | 0.192 ms | 0.049 ms | 1.204 ms | 0.056 ms | 0.265 ms | 220 | 0.942 ms | 1.582 ms | 0.000 ms | 2.539 ms |

Findings:

- Native raw565 removes runtime preview resize from the cache-hit path and
  cuts average preview load total from 7.351 ms to 2.539 ms versus same-build
  raw RGB.
- The raw preview blitter no longer does per-frame RGB8 to RGB565 conversion
  for cached raw565 images. `custom_draw_us` p95 fell from 4.304 ms to
  1.204 ms in the real launcher held-scroll run.
- Fixed-index framebuffer captures for raw RGB and raw565 both show the same
  selected games, and the raw565 path still clears the full cabinet aperture on
  image changes.

Artifacts:

- `build/preview-scroll-profiles/NATIVE565-BEFORE-RAWRGB-20260611-real.*`
- `build/preview-scroll-profiles/NATIVE565-AB-RAWRGB-20260611-real.*`
- `build/preview-scroll-profiles/NATIVE565-AFTER-RAW565-20260611-real.*`
- `build/preview-scroll-profiles/NATIVE565-VISUALS-20260611/`
- `build/preview-rgb565-samples/preview-rgb565-samples-20260611/`
