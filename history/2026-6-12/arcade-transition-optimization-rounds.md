# Arcade transition optimization rounds - 2026-06-12

This file tracks measured screenshot-transition optimization rounds after the
initial 34-effect implementation. Each committed round has a MiSTer benchmark
before/after comparison.

## Baseline scan

Command:

```bash
scripts/profile-preview-transition-mega.sh TRANSITIONS-R16-SCAN-20260612 --deploy-fast --segment-secs 3 --transition-ms 220 --fb-format 565 --preview-format raw-rgb565 --visual-captures 0
```

Overall result:

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| transition mega scan | 3858 | 26164 | 41578 | 1988 | 1906 | 32388 |

Worst initial transition targets:

| effect | frames | avg wall us | p95 wall us | >20 ms |
|---|---:|---:|---:|---:|
| clock-wipe | 62 | 49399 | 71749 | 39 |
| barn-door | 80 | 37237 | 44696 | 70 |
| iris | 80 | 38505 | 44496 | 68 |
| venetian-blinds | 80 | 37158 | 43551 | 70 |
| copper-bars | 79 | 37558 | 42924 | 70 |

## Round 16 - Integer clock-wipe angle gate

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-CLOCK-R16-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition clock-wipe --transition-ms 220 --visual-captures 0
```

This replaces the per-pixel floating-point `atan2` clock angle with a cheap
integer quadrant/ratio approximation. The effect keeps a radial clock-wipe
shape while removing libm work from the inner loop.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| transition mega scan clock segment | 62 | 49399 | 71749 | 39 | 39 | 32388 |
| round 16 focused clock-wipe | 324 | 36745 | 45174 | 270 | 270 | 23768 |

`clock-wipe` is still too expensive for 60fps, but this is a large measured
first cut and removes the worst per-pixel operation.

## Round 17 - Direct barn-door transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-BARN-R17-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition barn-door --transition-ms 220 --visual-captures 0
```

This moves `barn-door` out of the generic gate/blend path and into a direct
binary current/previous selection in the RGB565 transition loop.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| transition mega scan barn-door segment | 80 | 37237 | 44696 | 70 | 70 | 32388 |
| round 17 focused barn-door | 721 | 16386 | 16516 | 15 | 1 | 28076 |

`barn-door` is now inside the p95 60fps target.

## Round 18 - Direct venetian-blinds transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-VENETIAN-R18-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition venetian-blinds --transition-ms 220 --visual-captures 0
```

This moves `venetian-blinds` out of the generic transition gate and into a
direct binary current/previous selection in the RGB565 loop.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| transition mega scan venetian-blinds segment | 80 | 37158 | 43551 | 70 | 70 | 32388 |
| round 18 focused venetian-blinds | 721 | 16374 | 16462 | 14 | 1 | 28644 |

`venetian-blinds` is now inside the p95 60fps target.

## Round 19 - Direct tile-loader transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-TILE-LOADER-R19-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition tile-loader --transition-ms 220 --visual-captures 0
```

This moves `tile-loader` into a direct RGB565 binary selection arm using the
same tile hash as the generic gate.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| transition mega scan tile-loader segment | 88 | 33096 | 34398 | 88 | 88 | 32388 |
| round 19 focused tile-loader | 721 | 16368 | 16545 | 16 | 1 | 28644 |

`tile-loader` is now inside the p95 60fps target.

## Round 20 - Direct mask-blit transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-MASK-BLIT-R20-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition mask-blit --transition-ms 220 --visual-captures 0
```

This moves `mask-blit` into a direct RGB565 binary selection arm using the same
bit-mask expression as the generic transition gate.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| transition mega scan mask-blit segment | 97 | 32360 | 33764 | 96 | 96 | 32388 |
| round 20 focused mask-blit | 720 | 16391 | 16515 | 14 | 1 | 28644 |

`mask-blit` is now inside the p95 60fps target.

## Round 21 - Direct sprite-multiplex transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-SPRITE-MULTIPLEX-R21-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition sprite-multiplex --transition-ms 220 --visual-captures 0
```

This moves `sprite-multiplex` into a direct RGB565 binary selection arm using
the same multiplex hash as the generic gate.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| transition mega scan sprite-multiplex segment | 88 | 33306 | 34629 | 88 | 88 | 32388 |
| round 21 focused sprite-multiplex | 720 | 16386 | 16489 | 16 | 1 | 28712 |

`sprite-multiplex` is now inside the p95 60fps target.

## Round 22 - Direct row-scroll-parallax transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-ROW-SCROLL-R22-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition row-scroll-parallax --transition-ms 220 --visual-captures 0
```

This moves `row-scroll-parallax` into a direct RGB565 binary selection arm using
the same row phase as the generic gate.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| transition mega scan row-scroll-parallax segment | 88 | 32862 | 36105 | 80 | 80 | 32388 |
| round 22 focused row-scroll-parallax | 720 | 16391 | 16511 | 16 | 2 | 28712 |

`row-scroll-parallax` is now inside the p95 60fps target.

## Round 23 - Direct super-scaler-pop transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-SUPER-SCALER-R23-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition super-scaler-pop --transition-ms 220 --visual-captures 0
```

This moves `super-scaler-pop` into a direct RGB565 arm. It still uses the same
integer distance mask and alpha blend as the generic gate, but avoids the
generic gate/blend/decorate path.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| post-R22 mega super-scaler-pop segment | 70 | 41780 | 51631 | 54 | 54 | 32588 |
| round 23 focused super-scaler-pop | 721 | 16356 | 16476 | 21 | 1 | 28172 |

`super-scaler-pop` is now inside the p95 60fps target.
