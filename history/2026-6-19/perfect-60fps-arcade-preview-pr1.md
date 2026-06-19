# Perfect 60fps Arcade Preview PR1 Evidence

Date: 2026-06-19

Purpose: complete the profiling harness before runtime optimization. This PR
does not intentionally change launcher rendering behavior.

## Commands

Before/after preview pacing:

```bash
scripts/profile-preview-scroll.sh 60 held-scroll PR1-*-FADE-VEL --skip-build --transition fade --visual-captures 0
scripts/profile-preview-scroll.sh 60 turbo-hold PR1-*-FADE-TURBO --skip-build --transition fade --visual-captures 0
scripts/profile-preview-scroll.sh 60 held-scroll PR1-*-CUT-VEL --skip-build --transition cut --visual-captures 0
scripts/profile-preview-transition-mega.sh PR1-*-MEGA --skip-build --segment-secs 5 --transition-ms 320
scripts/profile-blend-velocity.sh 30 PR1-*-BLEND-BASE baseline --skip-build
scripts/profile-blend-velocity.sh 30 PR1-*-BLEND-COPY copy-only --skip-build
scripts/profile-blend-velocity.sh 30 PR1-*-BLEND-NOFADE no-fade --skip-build
```

CPU profile after the harness change:

```bash
MISTER_ALLOW_PREVIEW_HOTPATH_MISSES=1 scripts/profile-preview-scroll.sh 60 held-scroll PR1-AFTER-CPU-FADE-VEL --cpu-profile --transition fade --visual-captures 0
```

## Preview Pacing

These values are from the script's `frame_pacing` definition after frame 30:
`work_us = prepare_us + slint_render_us + custom_draw_us + fb_present_us`.

| Label | Frames | Exact | p99 work us | Work >16.7ms | p99 wall us | Wall >16.7ms | Vsync/fallback/timeout/error | Max miss streak |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: |
| PR1-BEFORE-FADE-VEL | 3595 | 3146 | 14106 | 8 | 16946 | 71 | 3564/0/0/0 | 0 |
| PR1-AFTER-FADE-VEL | 3597 | 3148 | 14101 | 4 | 16932 | 68 | 3566/0/0/0 | 0 |
| PR1-BEFORE-FADE-TURBO | 3595 | 3018 | 14127 | 0 | 16946 | 108 | 3564/0/0/0 | 0 |
| PR1-AFTER-FADE-TURBO | 3595 | 3018 | 14140 | 1 | 16944 | 105 | 3564/0/0/0 | 0 |
| PR1-BEFORE-CUT-VEL | 3598 | 3149 | 2979 | 0 | 16964 | 427 | 3567/0/0/0 | 0 |
| PR1-AFTER-CUT-VEL | 3598 | 3149 | 2992 | 0 | 16975 | 424 | 3567/0/0/0 | 0 |
| PR1-BEFORE-MEGA | 10196 | 9035 | 14059 | 3 | 16872 | 126 | 10165/0/0/0 | 0 |
| PR1-AFTER-MEGA | 10196 | 9035 | 14055 | 4 | 16893 | 124 | 10165/0/0/0 | 0 |
| PR1-AFTER-CPU-FADE-VEL | 3602 | 3153 | 14216 | 1 | 16944 | 62 | 3571/0/0/0 | 0 |

Notes:

- Harness-only change did not materially move p99 work or vsync source counts.
- The `transition-mega` run only reported `fade` in the per-effect summary; this
  needs follow-up before using mega as evidence across every transition.
- Wall-over-budget frames remain dominated by the wait/pacing boundary; true
  work misses are the acceptance signal for PR2.

## Blend Isolation

30s `profile-blend-velocity` result lines:

| Label | Variant | Frames | Avg fade blend us | Avg fade copy us | Avg body copy us | Avg wall us |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| PR1-BEFORE-BLEND-BASE | baseline | 1801 | 581 | 118 | 388 | 16485 |
| PR1-AFTER-BLEND-BASE | baseline | 1801 | 581 | 120 | 391 | 16481 |
| PR1-BEFORE-BLEND-COPY | copy-only | 1801 | 0 | 123 | 357 | 16490 |
| PR1-AFTER-BLEND-COPY | copy-only | 1800 | 0 | 123 | 359 | 16499 |
| PR1-BEFORE-BLEND-NOFADE | no-fade | 1801 | 0 | 0 | 474 | 16494 |
| PR1-AFTER-BLEND-NOFADE | no-fade | 1801 | 0 | 0 | 475 | 16495 |

## CPU Profile Artifact

The new CPU profile path produced:

- `build/preview-scroll-profiles/PR1-AFTER-CPU-FADE-VEL-arcade-cpu.svg`
- 283387-byte remote SVG, 277 KB local file.
- 2144 sample hits, 286 unique stacks, 60.3s at 99 Hz.

The flamegraph names hot supervised Arcade path functions including
`blit_transition_565_fade`, `copy_cached_rect_565`,
`build_launcher_present_plan`, `PreviewWorker::drain`, and
`load_raw565_preview_timed`.
