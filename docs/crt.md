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

| Resolved mode | RGB565 framebuffer → scan | Pixel clock | Horizontal timing (active/front/sync/back) | Vertical timing (active/front/sync/back) | Nominal rates |
| --- | --- | ---: | --- | --- | --- |
| `crt-240p60` | 320×240 → 640×240 | 12.587 MHz | 640/30/60/70 | 240/4/4/14 | 15.7338 kHz / 60.052 Hz |
| `crt-288p50` | 384×288 → 640×288 | 12.587 MHz | 640/30/60/70 | 288/6/4/14 | 15.7338 kHz / 50.429 Hz |
| `crt-480p60` | 640×480 → 640×480 | 25.175 MHz | 640/16/96/48 | 480/8/4/33 | 31.4688 kHz / 59.940 Hz |
| `crt-576p50` | 620×480 → 640×576 | 25.175 MHz | 640/16/96/48 | 576/2/4/42 | 31.4688 kHz / 50.431 Hz |

Both sync polarities are negative. These values come from Main's standard
Menu Direct Video table; MagiK consumes the resolved name only to choose its
framebuffer and scan geometry. It does not synthesize or alter those timings.

Framebuffer dimensions, scan timing, and destination placement are separate.
The framebuffer dimensions describe RGB565 memory. The scan timing describes
the analogue raster owned by Main and Menu. The inclusive destination
rectangle places the framebuffer inside Menu's scan space:

| Mode | RGB565 framebuffer | Nominal scan | Destination rectangle |
| --- | --- | --- | --- |
| `crt-240p60` | 320×240 | 640×240 | `(67,706,12,251)` |
| `crt-288p50` | 384×288 | 640×288 | `(67,706,32,286)` |
| `crt-480p60` | 640×480 | 640×480 | `(45,684,31,510)` |
| `crt-576p50` | 620×480 | 640×576 | `(45,664,40,615)` |

The 288p rectangle is the attended USB Video calibration result. The 576p
launcher uses matching 620-pixel framebuffer and destination widths. Earlier
trials narrowed only the destination while retaining a 640-pixel source, which
cropped source columns because Menu disables framebuffer downscaling. Matching
the source and destination widths instead preserves the complete Slint layout
while reducing its analogue width.

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

The bounded `crt_trial` scene exercises RGB565 publication through the shared
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

CRT support remains unqualified until a later attended real-CRT gate verifies
the exact candidate on representative displays, alongside HDMI regression,
core launch/return (including native interlace), cleanup, recovery, and stock
rollback. No current document should be read as claiming that gate has passed.
