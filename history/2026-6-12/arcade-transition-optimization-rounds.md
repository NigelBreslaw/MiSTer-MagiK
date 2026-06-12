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

## Round 24 - Direct venetian-copper transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-VENETIAN-COPPER-R24-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition venetian-copper --transition-ms 220 --visual-captures 0
```

This moves `venetian-copper` into a direct RGB565 selection arm. It preserves
the venetian/copper reveal and bright scanline flourish while avoiding the
generic gate and nested decoration match.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| post-R22 mega venetian-copper segment | 78 | 39974 | 50961 | 60 | 60 | 32588 |
| round 24 focused venetian-copper | 721 | 16367 | 16522 | 21 | 2 | 28716 |

`venetian-copper` is now inside the p95 60fps target.

## Round 25 - Direct copper-bars transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-COPPER-BARS-R25-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition copper-bars --transition-ms 220 --visual-captures 0
```

This moves `copper-bars` into a direct RGB565 selection arm. It preserves the
horizontal copper-bar reveal and bright scanline flourish while avoiding the
generic gate and decoration path.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| post-R22 mega copper-bars segment | 79 | 38660 | 44704 | 67 | 67 | 32588 |
| round 25 focused copper-bars | 720 | 16392 | 16493 | 17 | 1 | 28712 |

`copper-bars` is now inside the p95 60fps target.

## Round 26 - Direct starfield-warp transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-STARFIELD-WARP-R26-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition starfield-warp --transition-ms 220 --visual-captures 0
```

This moves `starfield-warp` into a direct RGB565 arm. It preserves the distance
plus hash-noise reveal and bright star sparkle while avoiding the generic
gate/blend/decorate path.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| post-R25 mega starfield-warp segment | 71 | 41224 | 52061 | 54 | 54 | 32216 |
| round 26 focused starfield-warp | 717 | 16424 | 18181 | 147 | 6 | 27892 |

`starfield-warp` is much closer to the 60fps target, though it still needs a
second pass for p95.

## Round 27 - Direct vector-redraw transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-VECTOR-REDRAW-R27-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition vector-redraw --transition-ms 220 --visual-captures 0
```

This moves `vector-redraw` into a direct RGB565 arm. It preserves the diagonal
redraw gate and sparse vector-line reveal while avoiding the generic gate path.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| post-R25 mega vector-redraw segment | 72 | 39340 | 49389 | 54 | 54 | 32216 |
| round 27 focused vector-redraw | 721 | 16358 | 16497 | 19 | 2 | 28048 |

`vector-redraw` is now inside the p95 60fps target.

## Round 28 - Direct palette-cycle transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-PALETTE-CYCLE-R28-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition palette-cycle --transition-ms 220 --visual-captures 0
```

This moves `palette-cycle` into a direct RGB565 arm. It preserves the alternating
half-alpha palette blocks and sparse brightened pixels while avoiding the generic
gate and decoration path.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| post-R27 mega palette-cycle segment | 86 | 36487 | 45386 | 66 | 66 | 32892 |
| round 28 focused palette-cycle | 681 | 17343 | 19796 | 346 | 28 | 26864 |

`palette-cycle` is much closer to the 60fps target, though it still needs a
second pass for p95.

## Round 29 - Direct plasma-mask transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-PLASMA-MASK-R29-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition plasma-mask --transition-ms 220 --visual-captures 0
```

This moves `plasma-mask` into a direct RGB565 binary selection arm using the
same plasma gate as the generic path.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| post-R27 mega plasma-mask segment | 80 | 38799 | 44935 | 70 | 70 | 32892 |
| round 29 focused plasma-mask | 664 | 17778 | 19752 | 599 | 26 | 26740 |

`plasma-mask` is much closer to the 60fps target, though it still needs a second
pass for p95.

## Round 30 - Direct moire-rings transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-MOIRE-RINGS-R30-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition moire-rings --transition-ms 220 --visual-captures 0
```

This moves `moire-rings` into a direct RGB565 binary selection arm using the
same distance-ring gate as the generic path.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| post-R27 mega moire-rings segment | 88 | 33862 | 44254 | 73 | 73 | 32892 |
| round 30 focused moire-rings | 648 | 18218 | 20551 | 641 | 73 | 27508 |

`moire-rings` is much closer to the 60fps target, though it still needs a
second pass for p95.

## Round 31 - Direct phosphor-decay transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-PHOSPHOR-DECAY-R31-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition phosphor-decay --transition-ms 220 --visual-captures 0
```

This moves `phosphor-decay` into a direct RGB565 arm. It preserves the top-down
reveal and dimming tail while avoiding the generic gate and decoration path.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| post-R27 mega phosphor-decay segment | 79 | 37293 | 43343 | 70 | 70 | 32892 |
| round 31 focused phosphor-decay | 720 | 16358 | 19127 | 179 | 5 | 27956 |

`phosphor-decay` is much closer to the 60fps target, though it still needs a
second pass for p95.

## Round 32 - Direct iris transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-IRIS-R32-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition iris --transition-ms 220 --visual-captures 0
```

This precomputes the iris radius threshold once per transition frame and moves
`iris` into a direct RGB565 binary selection arm.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| post-R27 mega iris segment | 87 | 34005 | 40475 | 70 | 70 | 32892 |
| round 32 focused iris | 720 | 16374 | 16588 | 31 | 4 | 28232 |

`iris` is now inside the p95 60fps target.

## Round 33 - Direct clock-wipe transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-CLOCK-WIPE-R33C-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition clock-wipe --transition-ms 220 --visual-captures 0
```

This moves `clock-wipe` into a direct RGB565 binary selection arm using the
same precomputed angle map as the generic path.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| post-R27 mega clock-wipe segment | 88 | 35078 | 40749 | 77 | 77 | 32892 |
| round 33 focused clock-wipe | 704 | 16790 | 18102 | 252 | 3 | 27952 |

`clock-wipe` is much closer to the 60fps target, though it still needs a
second pass for p95.

## Round 34 - Direct sprite-strips transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-SPRITE-STRIPS-R34-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition sprite-strips --transition-ms 220 --visual-captures 0
```

This moves `sprite-strips` into a direct RGB565 binary selection arm while
preserving the per-strip horizontal skew.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| post-R27 mega sprite-strips segment | 88 | 33150 | 38343 | 77 | 77 | 32892 |
| round 34 focused sprite-strips | 720 | 16392 | 16536 | 19 | 1 | 28776 |

`sprite-strips` is now inside the p95 60fps target.

## Round 35 - Direct racing-beam transition fast path

Command:

```bash
scripts/profile-preview-scroll.sh 12 held-scroll TRANSITION-RACING-BEAM-R35-20260612 --deploy-fast --fb-format 565 --preview-blitter raw --preview-format raw-rgb565 --transition racing-beam --transition-ms 220 --visual-captures 0
```

This moves `racing-beam` into a direct RGB565 arm while preserving the bright
lead beam and copper-row highlights.

| run | frames | avg wall us | p95 wall us | >16.7 ms | >20 ms | rss hwm kb |
|---|---:|---:|---:|---:|---:|---:|
| post-R27 mega racing-beam segment | 89 | 34146 | 35235 | 88 | 88 | 32892 |
| round 35 focused racing-beam | 720 | 16370 | 16539 | 25 | 1 | 28712 |

`racing-beam` is now inside the p95 60fps target.
