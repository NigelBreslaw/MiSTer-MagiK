# Mini-MagiK visual concepts

Device-only HDMI concepts use the existing Mini delivery and native test bridge.
The full launcher is not linked. No concept is device-qualified until its two
unprofiled windows, authoritative captures and physical animation review pass.

```
scripts/magik concept diagnostic
scripts/magik check concept --app mini-magik --concept diagnostic
```

The interactive session accepts `select NAME`, `preset default|reduced`, `pause`,
`resume`, `step`, `restart`, `capture`, and `quit`. Code changes require an
incremental rebuild; selection and presets do not. On `quit`, Ctrl-C, EOF or
connection loss, the native service restarts Mini with the last selected concept
and preset, playing from the beginning. This keeps the current work on HDMI,
following the requested iteration workflow. `scripts/magik stop` explicitly
returns to the ordinary launcher. Test control connections have a ten-minute
limit; the retained concept continues independently afterward.

Checks use two 30-second windows with two seconds of device-clock warmup each.
Separate `--profile` uses one ten-second attribution window. Streams and captures
must be off during qualification. Results retain binary identity, configuration,
physical cadence, CPU and RSS. Required gates: zero physical repeats and latch
failures, nominal 60 Hz, CPU below 150%, RSS at most 128 MiB.

## Implementation checklist

- [x] Physical cadence and CPU measurements
- [x] Portable contract and cached Mini session
- [x] Host controls and explicit concept checks
- [x] Shared fixtures
- [x] Point-cloud morph
- [x] Depth and parallax
- [x] Satin light sweep
- [x] Mirror floor removed after user review
- [x] Pixel dissolve
- [x] Starfield
- [x] Palette aurora removed after user review
- [x] Texture tunnel
- [x] Raster waves
- [x] Wireframe terrain removed after user review
- [ ] Physical qualification and visual review

`point-cloud-morph`: independently selectable; default and reduced presets.

`depth-parallax`: independently selectable; default and reduced presets.

`light-sweep`: a subtle card sheen with continuous translation; see the
[research and visual choices](light-sweep-research.md).


`pixel-dissolve`: independently selectable; default and reduced presets.

`starfield`: filtered stars with no comets; see the
[sampling diagnosis and research](starfield-research.md).


`texture-tunnel`: curved tube with a moving, banking camera; see the
[motion diagnosis and research](tunnel-research.md).

`raster-waves`: independently selectable; default and reduced presets.


## Presets and preparation

| Concept | Default | Reduced | Loop |
|---|---|---|---|
| point-cloud-morph | 8,192 points | 4,096 points | 20 s cabinet and logo |
| depth-parallax | Five cards, 25 cached poses per transition | Three-card crop, 13 poses | 16 s, forward then reverse |
| light-sweep | Quarter-pixel sheen profile, 6.25% peak | Half-pixel profile, same strength and speed | 3 s |
| pixel-dissolve | Eight-pixel tiles | 16-pixel tiles | 3.2 s, including endpoint holds |
| starfield | 256 filtered stars, no trails | 128 filtered stars, no trails | 8.192 s |
| texture-tunnel | 480×270, 480 palette-index frames, 256×256 texture | 120×67, 480 frames, same texture | 8 s forward flight, distance shading |
| raster-waves | Eight-pixel displacement | Four-pixel displacement | 2.048 s |

Working dimensions above describe the observed 960×540 render surface. Fixture
letterboxing is also tested at 960×600. Procedural backgrounds fill the surface.
Depth stores only the carousel rectangle, shares forward poses with reverse
playback, and blends adjacent poses. Reduced depth crops the outer two cards.
Dissolve prepares ordered tile thresholds and copies row spans. Tunnel preparation
projects a curved tube from a moving, banking camera with near-plane clipping
and perspective-correct texture coordinates. Each nominal display interval has
its own projected frame, stored as palette indices (59.3 MiB at the default
working dimensions). Playback converts those indices to RGB565 without blending
coordinates across changing wall visibility. Preparation and
preset changes occur outside measured windows. Presets never adapt automatically.

## Iteration and evidence

Run one named concept at a time. Checks perform two independent 30-second
windows after two-second warmups, then exercise both presets, switching, pause,
single-step and restart. Exact device timeline bookmarks stop rendering before
native scanout captures. Host command latency cannot move the captured timeline.
The point-cloud capture includes both cabinet and logo phases. The profile
command records a separate ten-second window and validates profile identity;
profiled runs cannot qualify cadence.

Results live under ignored `build/magik-results/<run-id>/`: raw measurement JSON,
`concept-results.json`, initial/midpoint/boundary PNGs with capture metadata,
events, logs and optional profile artifacts. Every measured result identifies
its binary SHA-256, preset, actual refresh and dimensions, samples, physical
repeats, latch failures, CPU, peak RSS, and render/transfer/presentation timings.
Streaming publication is disabled while a concept measurement is active.

The live HDMI animation review is performed by the user. The USB capture adapter
was unavailable during this implementation. Physical cadence comes from settled
protocol-v5 counters; framebuffer captures establish RGB565 appearance but do not
substitute for the live animation review.

See [recorded device qualification](visual-concepts-qualification.md) for measured
revisions, evidence directories, failed gates and remaining visual acceptance.
