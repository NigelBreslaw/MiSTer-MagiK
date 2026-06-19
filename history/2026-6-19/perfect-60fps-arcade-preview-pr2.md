# Perfect 60fps Arcade Preview PR2 Evidence

Date: 2026-06-19

Purpose: optimize the common RGB565 fade case where previous and current raw
preview frames have the same on-screen geometry.

## Change

- Added a same-geometry RGB565 fade path using direct destination/source row
  slices.
- Hoisted RGB565 alpha math into a row-local helper for the fast path.
- Preserved the generic fade fallback for empty frames, alpha endpoints,
  clipped/mismatched geometry, and non-RGB565 paths.
- Added equivalence coverage for alpha `1`, `64`, `128`, `192`, and `254`.

## Device Data

`PR1-AFTER-*` is the before set for this PR. `PR2-AFTER2-*` is after the final
row-local blend helper. Values use the script's `frame_pacing` definition after
frame 30.

| Label | Frames | Exact | p99 work us | Work >16.7ms | p99 wall us | Wall >16.7ms | Vsync/fallback/timeout/error | Max miss streak |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: |
| PR1-AFTER-FADE-VEL | 3597 | 3148 | 14101 | 4 | 16932 | 68 | 3566/0/0/0 | 0 |
| PR2-AFTER2-FADE-VEL | 3597 | 3148 | 14014 | 2 | 16937 | 64 | 3566/0/0/0 | 0 |
| PR1-AFTER-FADE-TURBO | 3595 | 3018 | 14140 | 1 | 16944 | 105 | 3564/0/0/0 | 0 |
| PR2-AFTER2-FADE-TURBO | 3598 | 3020 | 14079 | 0 | 16946 | 106 | 3567/0/0/0 | 0 |
| PR1-AFTER-CUT-VEL | 3598 | 3149 | 2992 | 0 | 16975 | 424 | 3567/0/0/0 | 0 |
| PR2-AFTER2-CUT-VEL | 3597 | 3148 | 2970 | 0 | 16967 | 425 | 3566/0/0/0 | 0 |
| PR2-AFTER2-CPU-FADE-VEL | 3602 | 3153 | 13790 | 3 | 16920 | 66 | 3571/0/0/0 | 0 |

Visual captures were collected for `PR2-AFTER2-FADE-VEL` and
`PR2-AFTER2-FADE-TURBO` under `build/preview-scroll-profiles/`.

## CPU Profile Artifact

- `build/preview-scroll-profiles/PR2-AFTER2-CPU-FADE-VEL-arcade-cpu.svg`
- 277926-byte remote SVG, 271 KB local file.
- 1937 sample hits, 276 unique stacks, 60.3s at 99 Hz.
- Flamegraph labels include `blit_transition_565_fade`,
  `blit_transition_565_fade_same_geometry`, `blend_565_row`,
  `copy_cached_rect_565`, and `build_launcher_present_plan`.

## Conclusion

PR2 improves the fade p99 work slightly and clears turbo true work misses in the
release build, but normal velocity still has two true work misses after frame
30. Proceed to PR3 blend math optimization.
