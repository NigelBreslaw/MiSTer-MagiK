# Mini-MagiK visual experiments

Five independently selectable RGB565 effects were accepted in live HDMI review:
light sheen, pixel dissolve, starfield, curved tunnel and raster waves. They run
in Mini-MagiK without linking the full launcher application. The shared fixture
uses existing card artwork, bitmap typography and fixed synthetic library data.

```sh
scripts/magik concept light-sweep
scripts/magik check concept --app mini-magik --concept light-sweep
scripts/magik check concept --app mini-magik --concept light-sweep --profile
```

## Controls and iteration

The interactive command accepts `select NAME`, `preset default|reduced`, `pause`,
`resume`, `step`, `restart`, `capture`, and `quit`. Selection and playback controls
need no rebuild. `step` advances one nominal 60 Hz interval; `capture` pauses and
saves the authoritative scanout image. Use `resume` afterward to continue.

Code changes use incremental rebuild/deploy. On quit, Ctrl-C, EOF or connection
loss, the native service restarts Mini with the last selected effect and preset,
playing from the beginning. The current work stays on HDMI. `scripts/magik stop`
explicitly returns to the ordinary launcher. Control connections have a ten-minute
limit; the retained effect continues independently afterward.

Rendering uses the resolved HDMI dimensions and reusable RGB565 hidden slots.
Preparation happens before measurement. Paused scenes stop presenting once their
requested frame is complete. No display-mode changes or automatic quality changes
are made. `diagnostic` is a small moving-line fixture for presentation tests.

## Presets

Working dimensions below describe the observed 960×540 output buffer; 960×600 is
also covered by portable tests. Fixtures are letterboxed; procedural scenes fill
the output. Use `--preset reduced` explicitly to select the reduced budget.

| Effect name | Default | Reduced | Loop |
|---|---|---|---|
| `light-sweep` | Quarter-pixel sheen profile, 6.25% peak | Half-pixel profile, same strength and speed | 3 s |
| `pixel-dissolve` | Eight-pixel tiles | 16-pixel tiles | 3.2 s including endpoint holds |
| `starfield` | 256 filtered stars, no trails | 128 filtered stars, no trails | 8.192 s |
| `texture-tunnel` | 480×270, 480 palette-index frames | 120×67, 480 frames | 8 s |
| `raster-waves` | Eight-pixel displacement | Four-pixel displacement | 2.048 s |

The tunnel projects a curved tube from a moving, banking camera with near-plane
clipping and perspective-correct texture coordinates. Each nominal display frame
has its own geometry-correct image. Its default cache occupies 59.3 MiB and takes
approximately 32 seconds to prepare on the A9. Mini concept startup allows 60
seconds; ordinary startup retains its 20-second limit.

## Measurement and review

Checks run two unprofiled 30-second windows, each following two seconds of warmup
from the same initial timeline. Gates require zero protocol-v5 physical repeats,
zero latch drops/rejections, valid ownership, nominal 60 Hz, average whole-process
CPU below 150%, and peak RSS at most 128 MiB. A reduced pass cannot qualify default.
The separate ten-second profile is attribution evidence only. Streaming and
captures stay off during measurement; captures and playback checks run afterward.

Ignored `build/magik-results/<run-id>/` directories contain binary identity,
preset, geometry, raw measurements, timings, captures, logs and optional profiles.
The user reviews motion directly on HDMI; static captures do not establish motion
quality. See the [qualification report](visual-concepts-qualification.md) for
recorded binaries, results and validation limits.

Mirror floor, palette aurora, wireframe terrain, point-cloud morph, and depth and
parallax were removed after review. Combined effects, a controller gallery, CRT,
desktop previews and full-app integration remain outside these experiments.
