# CRT and Direct Video output

MiSTer MagiK does not implement a CRT raster. The launcher publishes RGB565
frames through the same `UIO_SET_FBUF`/`LFB` machinery used by Menu on HDMI.
Main and Menu's `sys_top` exclusively own the output clock, raster, sync, and
Direct Video mux. The MagiK RBF delta remains the protocol-v2 vblank latch; it
contains no CRT PLL, DDR scanout reader, line buffers, raster generator, or
output-clock mux.

## Launcher modes

The maintained Main fork supports two ways to activate the shared CRT path.
`direct_video=2` uses MiSTer's known HDMI-DAC detection and falls back to
`hdmi` when no supported DAC is found. Explicit native Analog IO VGA modes use
`direct_video=1`; `menu_pal` and `forced_scandoubler` then select one of the
four built-in progressive Menu modes:

| Resolved mode | 640×480 composition → RGB565 scanout | Pixel clock | Horizontal timing (active/front/sync/back) | Vertical timing (active/front/sync/back) | Nominal rates |
| --- | --- | ---: | --- | --- | --- |
| `crt-240p60` | 640×480 → 640×240 | 12.587 MHz | 640/30/60/70 | 240/4/4/14 | 15.7338 kHz / 60.052 Hz |
| `crt-288p50` | 640×480 → 640×288 | 12.587 MHz | 640/30/60/70 | 288/6/4/14 | 15.7338 kHz / 50.429 Hz |
| `crt-480p60` | 640×480 → 640×480 | 25.175 MHz | 640/16/96/48 | 480/8/4/33 | 31.4688 kHz / 59.940 Hz |
| `crt-576p50` | 640×480 → 640×576 | 25.175 MHz | 640/16/96/48 | 576/2/4/42 | 31.4688 kHz / 50.431 Hz |

Both sync polarities are negative. These values come from Main's standard
Menu Direct Video table; MagiK consumes the resolved name only to choose its
scanout and scan geometry. It does not synthesize or alter those timings.

Composition dimensions, scanout dimensions, scan timing, and destination
placement are separate. Slint, Rust Arcade rows, screensavers, and overlays
converge in one cached 640×480 RGB565 composition. A centred nearest-row
vertical transform writes the mode-native scanout buffer while preserving all
640 horizontal pixels. The scan timing describes the analogue raster owned by
Main and Menu. The inclusive destination rectangle posts the complete native
scanout raster into Menu's scan space:

| Mode | Composition | RGB565 scanout/capture | Destination rectangle |
| --- | --- | --- | --- |
| `crt-240p60` | 640×480 | 640×240 | `(67,706,12,251)` |
| `crt-288p50` | 640×480 | 640×288 | `(67,706,12,299)` |
| `crt-480p60` | 640×480 | 640×480 | `(45,684,31,510)` |
| `crt-576p50` | 640×480 | 640×576 | `(45,684,40,615)` |

The FPGA OSD/framebuffer path is a direct scan overlay, not a general-purpose
product scaler. Exact 2× relationships such as 320×144→640×288 or
320×288→640×576 are therefore not the UI objective. MagiK performs the
vertical conversion in userspace and gives the FPGA a framebuffer that already
matches the complete destination raster. Authoritative framebuffer captures
are consequently 640×240, 640×288, 640×480, or 640×576, not 640×480 source
captures.

Route-owned safe areas affect content, not the full-screen background. The
288p master canvas reserves 34 composition rows at the top and 22 at the
bottom; the 576p canvas reserves 64 pixels at the right. The 240p and 480p
routes use the complete composition. The 15 kHz routes use larger 16-pixel body
text, 32-pixel headings, 2-pixel borders, and an 8-pixel grid so important
strokes survive conversion. The 31 kHz routes retain the fine 640×480 metrics.

An attended trial moving the 576p destination bottom from line 615 through line
607 did not move or remove the unstable coloured pixels observed on the final
physical raster row. A coordinated Main trial that transferred that row into
vertical blanking also left the fault unchanged. The evidence therefore rules
out framebuffer destination height and Main's transformed active/blanking split
as causes; the standard Main timing and full-height destination remain in use.

Fresh CRT configuration defaults to native Analog IO VGA at 240p60. The
installer separately offers automatic HDMI-DAC detection, native VGA at
288p50, 480p60 or 576p50, and HDMI-only output. It maps them onto the existing
`direct_video`, `menu_pal`, and
`forced_scandoubler` keys, preserves saved upgrade choices, and restores only
installer-owned keys from the pre-MagiK snapshot.

The 240p60 and 288p50 choices are labelled as 15 kHz modes. The 480p60 and
576p50 choices require separate confirmation that the attached VGA, multisync,
or other CRT explicitly supports 31 kHz. DAC detection cannot discover a
display's supported scan rates. An incompatible display may fail to lock or
show a rolling or distorted image; MagiK does not claim that an unsupported
scan rate is harmless on every CRT.

## Core handoff and interlace

The four names above describe the launcher only. Launching a game hands control
back to Main and loads the selected core RBF. The loaded core then owns its
native video timing independently of the launcher, including interlaced modes.
For example, the PlayStation core may output 480i without MagiK adding a 480i
launcher mode or converting the frame. MagiK must never retain a Menu timing or
framebuffer route override across that handoff.

## Qualification

The bounded `crt_trial` scene exercises composition conversion and native
RGB565 publication through the shared
protocol-v2 latch for exactly 30 seconds. It requires Main to report one of the
four standard CRT modes, advancing flip counters, and no presentation failure.
It never switches an FPGA output route.

Morph 4K's analog bridge is useful for checking the emitted clock, totals,
porches, sync widths, polarities, and horizontal/vertical rates without owning
a CRT. The version-2 qualification evidence records those external analyzer
measurements and binds them to exact app, Main, RBF, protocol, and platform
hashes. CI can validate the evidence shape and the RBF timing/CDC/stock-delta
reports, but neither CI nor Morph analysis proves compatibility with every
physical CRT.

Qualification requires authoritative native-raster framebuffer captures plus
native `scripts/agent capture usb-video` JPEG stills in all four modes. The CRT
trial stores a single post-trial sink frame rather than recording video. Text
must remain sharp and readable, proportions and safe areas must be correct, and
latch, visual, and FPGA drops must stay zero. HDMI regression, core
launch/return (including native interlace), cleanup, recovery, and stock
rollback remain part of the gate. No current document should be read as
claiming that attended gate has passed.
