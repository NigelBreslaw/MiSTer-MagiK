# Starfield: sampling diagnosis and researched revision

The user requested removal of the comets and reported wiggling, flickering stars.
The former implementation had four concrete sources of discontinuity:

1. `t.as_millis() / 8` quantized time. Nominal 60 Hz motion alternated between
   two and three depth units per frame instead of retaining fractional progress.
2. Integer perspective division snapped each star to one pixel independently on
   each axis. A diagonal trajectory therefore became a staircase.
3. Depth wrapped from the near plane to the far plane at nonzero brightness.
4. Stars overwrote each other when projected onto the same pixel.

These are source-backed causes that can contribute to the reported symptoms.
Zero physical dropped frames does not rule out sampling artifacts within those
frames. The user's live HDMI review remains the visual acceptance test.

## Research and chosen filter

[PBRT 4, Sampling Theory](https://www.pbr-book.org/4ed/Sampling_and_Reconstruction/Sampling_Theory)
describes aliasing from sampling continuous image information at discrete points.
[PBRT 4, Image Reconstruction, section 8.8.6](https://www.pbr-book.org/4ed/Sampling_and_Reconstruction/Image_Reconstruction#MitchellFilter)
describes the parameterized Mitchell–Netravali cubic-filter family, its separable
construction, and the tradeoff between blur and ringing.

This implementation selects the nonnegative cubic B-spline member, B=1 and C=0,
with support across four pixels on each axis. This is an explicit design choice,
not PBRT's default parameter pair. Its weights sum to one and its first moment
tracks the fractional position. Nonnegative weights avoid dark ringing around
bright points on a black background. The tradeoff is softer stars with at most a
4×4 footprint. This is per-star filtering, with no full-screen blur or trails.

Time now uses the nominal display timestamp with fractional depth and projection.
A smooth fade reaches zero on both sides of depth recycling. Fractional channel
contributions accumulate before one RGB565 conversion, so overlaps add light
rather than erase it. The reusable accumulation buffer is allocated during
preparation; only previous and current star footprints are touched each frame.

The seed, 8.192-second travel period, cyan/orange colour families and 256/128 star
budgets are retained. Total filtered energy is scaled by two to keep the spread
stars legible. That brightness compensation is a visual choice for review, not
a value prescribed by the research. The concept is now named `starfield`; the
comet trail renderer and `starfield-comets` selection are removed.

## Tests and device evidence

Focused tests check:

- Fractional projection progress and zero brightness across recycling.
- Subpixel energy conservation and centroid tracking before quantization.
- Actual RGB565 output over a two-pixel diagonal translation: at test energy 200,
  green-channel summed brightness varies by at most four encoded units; its centroid
  error stays below 0.08 pixel and backward steps below 0.04 pixel. This is a
  bounded test, not a claim of zero quantization error at all brightnesses.
- Additive overlaps, clipping, deterministic reset, exact loop return, and long
  timestamps at 960×540 and 960×600.
- The existing retained-frame oracle checks stale-pixel restoration.

See [the device report](visual-concepts-qualification.md) for paired unprofiled
windows, actual binary identity, physical cadence, CPU and RSS. The user accepted the
revised starfield on HDMI on 22 September 2026 and requested the next effect.
