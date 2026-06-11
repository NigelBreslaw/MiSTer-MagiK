# Review PR Benchmarks

## PR 3: Bound Preview Result Work Per Frame

Commit under test: `2b5d426` before benchmark-note amend.

Commands:

```bash
scripts/profile-preview-scroll.sh 30 held-scroll REVIEW-PR3-BEFORE-HELD-20260611 --deploy-fast --visual-captures 0
scripts/profile-preview-scroll.sh 30 held-scroll REVIEW-PR3-AFTER-HELD-20260611 --deploy-fast --visual-captures 0
scripts/profile-preview-scroll.sh 30 turbo-hold REVIEW-PR3-BEFORE-TURBO-20260611 --skip-build --visual-captures 0
scripts/profile-preview-scroll.sh 30 turbo-hold REVIEW-PR3-AFTER-TURBO-20260611 --skip-build --visual-captures 0
scripts/profile-preview-transition-mega.sh REVIEW-PR3-BEFORE-MEGA-20260611 --skip-build --segment-secs 3 --transition-ms 220
scripts/profile-preview-transition-mega.sh REVIEW-PR3-AFTER-MEGA-20260611 --skip-build --segment-secs 3 --transition-ms 220
```

Summary:

| Scenario | Before | After | Result |
| --- | ---: | ---: | --- |
| held-scroll p95 wall | 32318 us | 17245 us | improved |
| held-scroll frames >20 ms | 768 | 4 | improved |
| turbo-hold p95 wall | 17343 us | 17297 us | neutral |
| turbo-hold frames >20 ms | 5 | 5 | neutral |
| mega p95 wall | 16802 us | 16536 us | improved |
| mega frames >20 ms | 2 | 1 | neutral/improved |

Trace files are under `build/preview-scroll-profiles/`.

## PR 4: Clamp Dirty Rects In Signed Space

Commit under test: `3dcfb93` before benchmark-note amend.

Commands:

```bash
scripts/bench-toolchain.sh REVIEW-PR4-BEFORE-20260611 --device --replace-label --launcher-scenario held-scroll --scene-secs 15
scripts/bench-toolchain.sh REVIEW-PR4-AFTER-20260611 --device --replace-label --launcher-scenario held-scroll --scene-secs 15
```

Summary from `history/toolchain-bench/results.tsv`:

| Scenario | render_us | vsync_us | copy_us | rows_avg | fps | visual |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| before | 207 | 4963 | 2973 | 701 | 60 | capture failed |
| after | 210 | 2868 | 3180 | 701 | 60 | capture failed |

Both runs appended timing rows but failed the mid-run framebuffer capture gate
with `capture-fail,visual_ok=no`, so the comparison is useful for present timing
only. The code fix is covered by host unit tests for offscreen and clipped rects.
