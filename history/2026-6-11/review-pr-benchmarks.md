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
