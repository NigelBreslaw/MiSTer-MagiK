# Perfect 60fps Arcade Preview PR3 Evidence

Date: 2026-06-19

Purpose: finish the fade hot path after PR2 still had true work misses in the
normal velocity release run.

## Change

- Added RGB565 component-pair blend tables for the same-geometry preview fade
  fast path.
- The row helper computes the alpha bucket once and blends each pixel with
  table lookups instead of per-pixel multiplies.
- `UiFrameTarget::open()` warms the tables before the launcher loop starts.
- Kept scalar `blend_565()` as the reference path and added table-vs-scalar
  equivalence coverage across all raw alpha values on representative RGB565
  pixels.

## Preview Pacing

`PR2-AFTER2-*` is the before set for this PR. Values use the script's
`frame_pacing` definition after frame 30.

| Label | Frames | Exact | p99 work us | Work >16.7ms | p99 wall us | Wall >16.7ms | Vsync/fallback/timeout/error | Max miss streak |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: |
| PR2-AFTER2-FADE-VEL | 3597 | 3148 | 14014 | 2 | 16937 | 64 | 3566/0/0/0 | 0 |
| PR3-AFTER-FADE-VEL | 3597 | 3148 | 13610 | 0 | 16929 | 60 | 3566/0/0/0 | 0 |
| PR2-AFTER2-FADE-TURBO | 3598 | 3020 | 14079 | 0 | 16946 | 106 | 3567/0/0/0 | 0 |
| PR3-AFTER-FADE-TURBO | 3596 | 3019 | 13668 | 0 | 16941 | 109 | 3565/0/0/0 | 0 |
| PR2-AFTER2-CUT-VEL | 3597 | 3148 | 2970 | 0 | 16967 | 425 | 3566/0/0/0 | 0 |
| PR3-AFTER2-CUT-VEL | 3597 | 3148 | 2982 | 0 | 16969 | 428 | 3566/0/0/0 | 0 |
| PR3-AFTER-CPU-FADE-VEL | 3602 | 3153 | 13832 | 2 | 16930 | 64 | 3571/0/0/0 | 0 |

Notes:

- Release normal velocity and turbo fade now both have zero true work misses.
- The CPU-profile build still has profiler-overhead work misses; release
  binaries are the acceptance signal.
- The first PR3 cut run had one cold selected-copy work miss at frame 32; the
  rerun above is clean and used as the control row.

## Blend Isolation

30s `profile-blend-velocity` result lines after PR3:

| Label | Variant | Frames | Avg fade blend us | Avg fade copy us | Avg body copy us | Avg wall us |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| PR3-AFTER-BLEND-BASE | baseline | 1801 | 581 | 121 | 385 | 16484 |
| PR3-AFTER-BLEND-COPY | copy-only | 1801 | 0 | 127 | 353 | 16492 |
| PR3-AFTER-BLEND-NOFADE | no-fade | 1801 | 0 | 0 | 471 | 16489 |

## CPU Profile Artifact

- `build/preview-scroll-profiles/PR3-AFTER-CPU-FADE-VEL-arcade-cpu.svg`
- 259631-byte remote SVG, 254 KB local file.
- 2011 sample hits, 295 unique stacks, 60.3s at 99 Hz.
- Flamegraph labels include `blit_transition_565_fade`,
  `blit_transition_565_fade_same_geometry`, `blend_565_row`, `blend_bucket`,
  `Blend565Tables`, `copy_cached_rect_565`, and `build_launcher_present_plan`.

## Conclusion

PR3 meets the preservation-of-fade milestone for release Arcade preview:
normal velocity and turbo fade both have `work_gt_16_7ms=0`, clean vsync source
counts, `max_vsync_miss_streak=0`, and p99 work below 14500us.
