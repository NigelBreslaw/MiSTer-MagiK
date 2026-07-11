# Production zero-copy baseline — 2026-07-11

All measurements use the stock `5.15.1-MiSTer` kernel, the legacy
`mister_magik_plugin_probe` module without `/dev/mister-magik-scanout`, RGB565,
and the `fpga-vblank-latch-hidden` presenter. Each accepted trace contains
roughly 1,768 measured frames after warmup and has zero fallback, timeout,
error, miss-streak, alternation, visual-latch, or FPGA-drop evidence.

| Scene | Scenario | work-p99 runs | Median gate |
|---|---|---:|---:|
| Home | `home-repeat-hold` | 6,904 / 6,776 / 6,888 us | 6,888 us |
| Arcade | `turbo-hold`, `vsync-integrity` | 3,722 / 3,771 / 3,736 us | 3,736 us |
| Preview | `held-scroll` | 2,446 / 2,469 / 2,470 us | 2,469 us |

Home copy-p99 was 1,479 / 1,489 / 1,512 us. Arcade and preview traces show
hidden composition p99 around 1.7 ms, split between preview and Arcade layers.

Preview rows 1,079–1,086 select one catalog entry without an exact preview
asset. The benchmark now accepts an explicit bounded allowance for this known
fixture gap; the traces contain no invalid or stale images. This allowance is
separate from scanout integrity and is printed in the result.

Artifacts:

- `build/launcher-home-scroll-profiles/PROD-ZC-HOME-BEFORE-R[1-3]-20260711-*`
- `build/arcade-scroll-profiles/PROD-ZC-ARCADE-BEFORE-R[1-3]V-20260711-*`
- `build/preview-scroll-profiles/PROD-ZC-PREVIEW-BEFORE-R[1-3]V-20260711-*`

Qualification rule: each of three AFTER work-p99 samples for every scene must
be strictly lower than its scene's BEFORE median, with zero scanout-integrity
violations.
