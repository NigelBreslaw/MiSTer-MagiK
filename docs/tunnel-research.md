# Curved tunnel: motion diagnosis and research

The user reported a glitch visible in motion near the tight distant bend. The
previous implementation prepared 64 default camera poses over eight seconds and
interpolated their screen-space texture coordinates at playback time. A pixel
can see different parts of the curved wall in successive poses; interpolating
those coordinates does not reproduce the surface visible from the intermediate
camera position.

A numerical comparison against directly projected intermediate geometry found
coordinate errors approaching 128 texels in the 256-pixel repeating texture near
1.19 seconds. At nominal frame 71, the regression
`tight_bend_frame_matches_direct_geometry` found 18,222 differing working-image
pixels in the previous renderer. The corrected renderer matches that direct
projection exactly. This identifies a temporal interpolation error; it does not
establish that every motion artifact reported by the user has been eliminated.

## Sources and how they informed the correction

- Shade, Gortler, He and Szeliski, [Layered Depth Images, SIGGRAPH 1998](https://www.microsoft.com/en-us/research/publication/layered-depth-images/),
  describes retaining multiple surfaces along viewing rays when synthesizing
  new views. It provides the visibility context: a single visible coordinate
  per pixel cannot describe all surfaces exposed by camera motion. This renderer
  does not implement layered depth images.
- [PBRT, Triangle Meshes](https://www.pbr-book.org/4ed/Shapes/Triangle_Meshes)
  provides the triangle and barycentric-coordinate foundation for evaluating
  actual geometry. The existing software rasterizer already projects triangles,
  clips the near plane, depth-tests, and interpolates texture coordinates with
  perspective correction. The correction preserves that geometry at every
  nominal animation frame instead of interpolating separate rendered views.

The exact cache layout, 480-frame loop, resolution, camera path and palette are
engineering and visual choices for this CPU-only device; the papers do not
prescribe them. No new research-backed claim is made about the existing colours
or checker pattern.

## Implementation and tradeoff

Preparation now projects all 480 nominal 60 Hz frames in the eight-second loop.
Four texture materials and 64 distance-shading levels fit in a one-byte palette
index. Only one temporary coordinate/depth map is alive at once. The default
480×270 sequence occupies 62,208,000 bytes (59.3 MiB); reduced 120×67 occupies
3,859,200 bytes. At 960×600, default working geometry is 480×300 and its sequence
occupies 69,120,000 bytes. These figures exclude the process and output buffers.

Playback performs palette conversion and the existing RGB565 enlargement, with
no coordinate blending between views. Preparation takes longer and memory use
increases. Both remain explicit costs, measured separately from steady playback.
Geometry, banking, forward speed, wall pattern and colours are unchanged.

Portable tests cover the offending pose, nominal single-frame movement, exact
loop endpoints, reset, and damage correctness at both output geometries. Device
cadence and RSS evidence belong in the [qualification report](visual-concepts-qualification.md).
Static captures establish appearance only; the user reviews the motion on HDMI.

The user confirmed that the motion glitch was fixed and accepted this version
on 22 September 2026. Both device windows passed with 1,800 frames, zero physical
repeats or latch failures, 100.24% / 100.18% CPU and a maximum 68.4 MiB RSS.
