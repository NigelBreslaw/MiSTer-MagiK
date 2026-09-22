# Light sweep: research, implementation and review

The original implementation was an unvalidated visual approximation: sixteen
(default) or eight (reduced) images with a pale diagonal stripe. It had no
effect-specific research behind it. Its passing device cadence measurements did
not establish visual quality. The user rejected its appearance and smoothness,
and selected a subtle card sheen as the replacement on 22 September 2026.

## Sources consulted for the replacement

- [W3C CSS Images 3, linear gradients](https://www.w3.org/TR/css-images-3/#linear-gradients)
  defines a gradient using positions along a line. This supplies the useful 2D
  construction: a soft intensity profile sampled along a diagonal coordinate.
  Our implementation uses a prepared smooth cubic profile in Rust, not CSS.
- [W3C CSS Easing 1, linear easing](https://www.w3.org/TR/css-easing-1/#linear)
  defines output progress equal to input progress. The replacement translates
  one profile at constant speed using the full nominal frame timestamp, with
  fractional coordinate interpolation. It does not crossfade stationary bands.
- [Khronos OpenGL 3.0 specification, section 4.1.10, Dithering](https://registry.khronos.org/OpenGL/specs/gl/glspec30.pdf)
  describes selecting neighboring representable color values when quantizing
  framebuffer output. We apply that principle in software with a fixed 4×4
  ordered threshold pattern when lifting RGB565 channels. No OpenGL or GPU is
  involved; the pattern is fixed to the surface, not randomized per frame.
- [Physically Based Rendering, 4th edition, roughness and microfacet theory](https://www.pbr-book.org/4ed/Reflection_Models/Roughness_Using_Microfacet_Theory)
  explains how roughness spreads reflected light. This is background for a broad,
  soft highlight. We do not implement its BRDF, infer material normals from the
  artwork, or claim physically accurate satin/cabinet lighting.

These sources support established techniques. They do not prescribe the exact
opacity, width, slope, cycle duration, or whether this design looks good.

## Deliberate visual choices

- Peak white contribution: 6.25%, down from the old 37.5% stripe peak. Blending is
  performed in the existing encoded RGB565 channel space as a UI overlay.
- A single smooth cubic lobe, with half-width 28% of the card width, and a diagonal
  coordinate `x + y/4`. Its slope is zero at both its center and outer edges.
- Constant travel over three seconds. It starts and ends completely outside the
  card, so restarting the loop does not introduce a visible jump.
- Feathered coverage on the artwork area. The frame, labels, statistics and other
  cards remain byte-identical. This represents a restrained coating on the card
  face, not moving lights on the depicted cabinet.
- Default profile sampling: quarter-pixel intervals; reduced: half-pixel
  intervals. Both interpolate position every frame and retain the same strength,
  speed and shape. There are no cached animation-phase images.

The old default stored band centers roughly 34 pixels apart, then crossfaded
between them over 200 ms. This can broaden or split the moving highlight even
when all 60 frames are physically presented. The new implementation translates
the profile itself. This identifies a concrete source-level defect; it does not
prove that it was the only cause of the perceived unevenness on HDMI.

## Verification and acceptance

Focused tests cover both presets at 960×540 and 960×600: preserved text/chrome,
exact initial and loop-boundary images, adjacent active frames changing,
translated-profile intensity stability, and bounded RGB565 channel changes.
The retained-frame oracle checks stale-pixel removal and deterministic reset.
Preparation and all allocations remain outside frame rendering.

Device results and binary identity are recorded in
[the qualification report](visual-concepts-qualification.md). Hardware cadence
and live visual approval are separate. Keep light sweep on HDMI and ask the user
whether to keep or adjust it before showing another effect.
