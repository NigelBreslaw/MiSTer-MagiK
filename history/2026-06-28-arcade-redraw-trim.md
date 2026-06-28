# 2026-06-28 Arcade Redraw Trim

Production-only slice for row renderer churn during Arcade turbo scroll.

## Change

- Store cached row titles as `Arc<str>` instead of allocating a `String`.
- Borrow unclipped row titles in the common case instead of allocating.
- Hash only visible row fields for row redraw detection: title and new-badge
  state. Nonvisual identity fields still drive preview and launch behavior, but
  they do not affect row pixels.

## Benchmarks

Commands:

```bash
scripts/profile-arcade-scroll.sh ARCDRAW-BEFORE-20260628 --skip-build --secs 30
scripts/profile-preview-scroll.sh ARCDRAW-BEFORE-20260628 --skip-build --secs 30 --scenario turbo-hold --visual-captures 0
scripts/profile-arcade-scroll.sh ARCDRAW-AFTER-20260628 --skip-build --secs 30
scripts/profile-preview-scroll.sh ARCDRAW-AFTER-20260628 --skip-build --secs 30 --scenario turbo-hold --visual-captures 0
```

Trace files:

- `build/arcade-scroll-profiles/ARCDRAW-BEFORE-20260628-arcade-scroll.tsv`
- `build/arcade-scroll-profiles/ARCDRAW-AFTER-20260628-arcade-scroll.tsv`
- `build/preview-scroll-profiles/ARCDRAW-BEFORE-20260628-arcade.tsv`
- `build/preview-scroll-profiles/ARCDRAW-AFTER-20260628-arcade.tsv`

## Result

Preview-scroll custom draw metrics recomputed from TSV rows after the first
30 frames:

| Metric | Before | After |
| --- | ---: | ---: |
| `arcade_list_update_us` avg | 147 us | 145 us |
| `arcade_list_update_us` p95 | 521 us | 517 us |
| `arcade_list_update_us` p99 | 605 us | 589 us |
| `arcade_list_present_us` p95 | 580 us | 558 us |
| `preview_blit_us` p95 | 1570 us | 1571 us |

Arcade-scroll present metrics were flat to slightly worse
(`arcade_list_present_us` p95 567 us -> 570 us), so this commit should be read
as a small row-update churn trim, not a framebuffer-copy optimization.
