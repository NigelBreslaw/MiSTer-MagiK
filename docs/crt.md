# CRT and Direct Video output

MiSTer MagiK does not implement a CRT raster. The launcher publishes RGB565
frames through the same `UIO_SET_FBUF`/`LFB` machinery used by Menu on HDMI.
Main and Menu's `sys_top` exclusively own the output clock, raster, sync, and
Direct Video mux. The MagiK RBF delta uses the protocol-v5 atomic latch; it
contains no CRT PLL, DDR scanout reader, line buffers, raster generator, or
output-clock mux.

Main is also the sole writer of the complete `UIO_BUT_SW` framework word.
MagiK never toggles `CONF_VGA_FB` directly: doing so with a partial word would
erase Main's composite-sync, SoG, scaler, Direct Video, audio, and HDMI flags.
Framebuffer activation and recovery only publish RGB565 geometry and pixels;
Main enables the mux before spawning MagiK and restores it during handoff.

## Launcher modes

The maintained Main fork supports two ways to activate the shared CRT path.
`direct_video=2` uses MiSTer's known HDMI-DAC detection and falls back to
`hdmi` when no supported DAC is found. Explicit native Analog IO VGA modes use
`direct_video=1`; `menu_pal` and `forced_scandoubler` then select one of the
four built-in progressive Menu modes:

| Resolved mode | RGB565 composition → scanout | Pixel clock | Horizontal timing (active/front/sync/back) | Vertical timing (active/front/sync/back) | Nominal rates |
| --- | --- | ---: | --- | --- | --- |
| `crt-240p60` | 640×240 → 640×240 (legacy compatibility: 640×480) | 12.587 MHz | 640/30/60/70 | 240/4/4/14 | 15.7338 kHz / 60.052 Hz |
| `crt-288p50` | 640×288 → 640×288 | 12.587 MHz | 640/30/60/70 | 288/6/4/14 | 15.7338 kHz / 50.429 Hz |
| `crt-480p60` | 640×480 → 640×480 | 25.175 MHz | 640/16/96/48 | 480/8/4/33 | 31.4688 kHz / 59.940 Hz |
| `crt-576p50` | 640×576 → 640×576 | 25.175 MHz | 640/16/96/48 | 576/2/4/42 | 31.4688 kHz / 50.431 Hz |

Both sync polarities are negative. These values come from Main's standard
Menu Direct Video table; MagiK consumes the resolved name only to choose its
scanout and scan geometry. It does not synthesize or alter those timings.

Composition dimensions, scanout dimensions, scan timing, and destination
placement are separate. Slint, Rust Arcade rows, screensavers, and overlays
converge in one cached RGB565 composition owned by the resolved route. The
production CRT240 path now composes directly at 640×240, matching the PAL
routes so presentation and dirty-row mapping are identity operations. The
previous 640×480 CRT240 composition remains available as a volatile
compatibility policy (`MISTER_CRT240_COMPOSITION=legacy-480`) for visual A/B
review. The scan timing describes the analogue raster owned by Main and Menu.
The inclusive destination rectangle posts the complete native scanout raster
into Menu's scan space:

| Mode | Composition | RGB565 scanout/capture | Destination rectangle |
| --- | --- | --- | --- |
| `crt-240p60` | 640×240 (legacy: 640×480) | 640×240 | `(67,706,12,251)` |
| `crt-288p50` | 640×288 | 640×288 | `(67,706,12,299)` |
| `crt-480p60` | 640×480 | 640×480 | `(45,684,31,510)` |
| `crt-576p50` | 640×576 | 640×576 | `(45,684,40,615)` |

The FPGA OSD/framebuffer path is a direct scan overlay, not a general-purpose
product scaler. Exact 2× relationships such as 320×144→640×288 or
320×288→640×576 are therefore not the UI objective. MagiK gives the FPGA a
framebuffer that already matches the complete destination raster. The legacy
CRT240 policy is the only path that deliberately performs a 480→240
conversion. Authoritative framebuffer captures are consequently 640×240,
640×288, 640×480, or 640×576.

## Capture inspection views

The typed framebuffer capture preserves the authoritative scanout PNG and, for
authoritative 15 kHz `640×240` and `640×288` captures, also emits two host-side
inspection views. `STEM-raw-letterbox-4x3.png` places the unchanged source
raster on a black `640×480` canvas. `STEM-display-4x3.png` maps source rows to
the same `640×480` canvas with centered nearest-scanline sampling. This makes
the square-pixel and physical-aspect interpretations visible together without
changing the device capture or inventing blended colors. The Arcade-list
workflow is the deterministic typography fixture:

```text
scripts/agent device launcher capture-first-arcade --attended --output STEM
```

The first visual experiment is an attended A/B row-phase comparison:

```text
scripts/agent device launcher capture-crt-font-ab --attended --pair row-phase --output STEM
```

It captures the production centered odd-row sampler as A and a top-aligned
even-row sampler as B, then emits a side-by-side `1280×480` true-4:3 review
image. The experiment is volatile and the production baseline remains the
default when `MISTER_CRT_FONT_EXPERIMENT` is unset.

For composition A/B review, keep the Arcade list open and capture the same
fixture twice. Native 640×240 is the production baseline; the compatibility
capture uses the old 640×480 composition and centred row transform:

```text
scripts/agent device launcher restart --attended --crt240-composition legacy-480
scripts/agent device launcher capture-first-arcade --attended --output CRT240-legacy
scripts/agent device launcher restart --attended --crt240-composition native
scripts/agent device launcher capture-first-arcade --attended --output CRT240-native
```

Compare each raw capture with its generated `-raw-letterbox-4x3.png` and
`-display-4x3.png` companions. This isolates composition changes from the
physical 4:3 inspection transform; do not compare a letterboxed image against
the true-4:3 view as if they were the same pixel grid.

The font-only follow-up locks Arcade glyph coverage to absolute two-row groups
before the 480→240 conversion. Each pair receives the maximum alpha coverage
from either source row, while horizontal metrics, layout, and the backdrop stay
unchanged. Activate it without capturing so the operator can open the review
fixture manually:

```text
scripts/agent device launcher restart --attended --crt-font-experiment coverage-max
```

The `dominant-row` follow-up preserves one complete source row per absolute
two-row group instead of merging their pixels. It chooses the row with greater
total glyph coverage and prefers the production odd row on ties, then repeats
that row across the pair. Activate it for the next attended comparison:

```text
scripts/agent device launcher restart --attended --crt-font-experiment dominant-row
```

The `xerxes` arm replaces the CRT240 Arcade title typeface with the existing
precompiled Xerxes 10 bitmap resource. Its 16px renderer size produces exact
10-pixel capitals, and it uses the production centered sampler without any row
repair filter:

```text
scripts/agent device launcher restart --attended --crt-font-experiment xerxes
```

The `xerxes-perfect` arm uses Xerxes 10 at 32px. Its 64-unit design cells map
to exact 2×2 composition blocks, so centered 480→240 scanout retains every
cell as a 2×1 framebuffer block. Its 640-unit capitals occupy 20 composition
rows and 10 output scanlines.

```text
scripts/agent device launcher restart --attended --crt-font-experiment xerxes-perfect
```

The `yesterday-perfect` arm applies the same lossless 32px CRT240 mapping to
Yesterday 10. Its 64-unit design cells become exact 2×2 composition blocks,
and its 640-unit capitals become 20 composition rows and 10 output scanlines.

```text
scripts/agent device launcher restart --attended --crt-font-experiment yesterday-perfect
```

The `bacteria` arm uses the supplied Bacteria 12 bitmap design at 32px for the
CRT240 Arcade title and game rows. The font has a 1,024-unit em and 64-unit
design grid, so 32px maps each design cell to a 2×2 composition block. The
production 480→240 transform therefore preserves every cell as an exact 2×1
framebuffer block, which is physically square at 640×240 on a 4:3 display. Its
768-unit capitals become 24 composition rows and 12 output scanlines:

```text
scripts/agent device launcher restart --attended --crt-font-experiment bacteria
```

The `bacteria-half` arm uses the same supplied design at its native 16px size.
Its capitals occupy 12 composition rows and therefore approximately six output
scanlines after the unchanged centered 480→240 conversion. This is the direct
half-size comparison: it applies no row reconstruction and may lose alternating
design rows by construction.

```text
scripts/agent device launcher restart --attended --crt-font-experiment bacteria-half
```

The production CRT typography uses Jersey 25 for major headings, native
Terminus 8×14 for settings, status, and small text, and Nocive 15 for footer
hints. Press Start 2P remains available to HDMI but is not used by the CRT UI.
CRT240 Arcade game titles use the pixel-perfect 32px Yesterday 10 resource;
the retained Terminus 28px normal and bold resources are not Arcade title
selectors.

Only `STEM-raw.png` is authoritative framebuffer evidence; the two `4x3`
files are derived host previews and must not be used as HDMI/CRT sink proof.

Route-owned safe areas affect content, not the full-screen background. The
288p canvas reserves 20 native rows at the top and 13 at the bottom, producing
a 640×255 content rectangle; the 576p canvas reserves 64 pixels at the right.
The 240p and 480p routes use the complete composition. The 288p route retains
the 15 kHz horizontal metrics but uses 5-line vertical grid units, 2×1
axis-specific border tokens, 14-line rows, a 29-line header, and a 24-line
footer. The 576p route uses 4×5 grid units, 1-line borders, 29-line rows, a
38-line header, and a 29-line footer. The 60 Hz routes remain unchanged.

PAL text uses OFL-derived Press Start assets with horizontal advances
unchanged and glyph outlines plus vertical metrics scaled by 3:5 for 288p and
6:5 for 576p. Slint and the Rust Arcade renderer select the same route-owned
family. This compensates for the physical PAL pixel aspect without relying on
an anisotropic Slint software-renderer transform.

Hidden RGB565 slot publication has a strict copy → overlay → publish → latch
order. On ARM, `publish_writes()` issues a full-system store barrier after
write-combined framebuffer writes and before posting the FPGA latch. Route
fallback and fixed-animation pacing use Main's resolved periods: 16,652 µs
for 240p, 19,830 µs for 288p, 16,683 µs for 480p, and 19,829 µs for 576p.

## Cold-catalog intro

All four resolved CRT routes support the 20-second cold-catalog particle intro
through an intro-specific native hidden-slot grant. This capability does not
broaden direct screensaver eligibility. CRT uses 51,200 initial and 20,480
steady particles, exactly half the HDMI density, with deterministic per-letter
track thinning so MiSTer-to-MagiK identities remain paired.

The authored 16:9 scene is fully visible and centred within the physical 4:3
raster. Projection uses X scale `2/3` and Y scale
`native_framebuffer_height/720`; inverse scales generate pixel-exact launcher
morph targets. The scene renders at 640×240, 640×288, 640×480, or 640×576. For
240p only, the live launcher target is derived from the retained 640×480
composition with the standard centred nearest-row transform. The original
composition cache is restored after the final native frame.

Storyboard time advances after each confirmed presentation using the resolved
refresh period and clamps to exactly 20 seconds, so PAL routes do not stretch
the sequence. Physical frame numbering remains independent. Preparation,
transform, grant, route, or latch failure reveals the ordinary launcher rather
than leaving the intro in control.

An attended trial moving the 576p destination bottom from line 615 through line
607 did not move or remove the unstable coloured pixels observed on the final
physical raster row. A coordinated Main trial that transferred that row into
vertical blanking also left the fault unchanged. A subsequent write-publication
barrier trial did not eliminate the larger lower-screen motion glitch visible
on the physical TV. Static USB Video output remained clean. This narrows the
fault to a motion-sensitive PAL path but does not prove a Main, FPGA, display,
or application cause. The standard Main timing and full-height destinations
remain in use while native PAL composition is qualified.

Installation does not select or alter an output route. It preserves the active
`video_mode`, `direct_video`, `menu_pal`, and `forced_scandoubler` values
byte-for-byte. The launcher resolves the existing route from Main's runtime
state and the effective `[Menu]`, then `[MiSTer]`, configuration.

Explicit output changes remain attended operations. In particular, DAC
detection cannot discover a display's supported scan rates. An incompatible
31 kHz setting may fail to lock or show a rolling or distorted image; MagiK
does not claim that an unsupported scan rate is harmless on every CRT.

## Core handoff and interlace

The four names above describe the launcher only. Launching a game hands control
back to Main and loads the selected core RBF. The loaded core then owns its
native video timing independently of the launcher, including interlaced modes.
For example, the PlayStation core may output 480i without MagiK adding a 480i
launcher mode or converting the frame. MagiK must never retain a Menu timing or
framebuffer route override across that handoff.

## Qualification

The bounded `crt_trial` scene exercises route-owned composition and native
RGB565 publication through the shared
protocol-v5 latch for exactly 30 seconds. It requires Main to report one of the
four standard CRT modes, advancing flip counters, and no presentation failure.
Matching top and bottom frame markers expose mixed-frame publication
immediately. It never switches an FPGA output route.

Morph 4K's analog bridge is useful for checking the emitted clock, totals,
porches, sync widths, polarities, and horizontal/vertical rates without owning
a CRT. The version-3 qualification evidence records those external analyzer
measurements and binds them to exact app, Main, RBF, protocol, and platform
hashes. CI can validate the evidence shape and the RBF timing/CDC/stock-delta
reports, but neither CI nor Morph analysis proves compatibility with every
physical CRT.

Qualification requires authoritative native-raster framebuffer captures plus
native `scripts/agent capture usb-video` JPEG stills in all four modes. The CRT
trial stores a single post-trial sink frame rather than recording video. Text
must remain sharp and readable, proportions and safe areas must be correct, and
latch, visual, and FPGA protocol failures must stay zero. Motion cadence must
also show zero physical refreshes that reused the previous confirmed frame;
zero latch or FPGA drops alone is not evidence of zero dropped frames. HDMI regression,
core launch/return (including native interlace), cleanup, recovery, and stock
rollback remain part of the gate. No current document should be read as claiming
that attended gate has passed. CRT intro implementation and host assurance are
present; four-mode installed-device cadence and visual qualification remain
pending until the delivery and attended review complete.
