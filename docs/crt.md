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
| `crt-576p50` | 640×480 → 640×576 | 25.175 MHz | 640/16/96/48 | 576/2/4/42 | 31.4688 kHz / 50.431 Hz |

Both sync polarities are negative. These values come from Main's standard
Menu Direct Video table; MagiK consumes the resolved name only to choose its
framebuffer and scan geometry. It does not synthesize or alter those timings.
The HPS framebuffer destination remains distinct from the raster: 288p is
inset vertically into a 640×272 destination within the unchanged 640×288
raster, and 576p uses the full 640-pixel destination width from horizontal
origin zero. The 240p and 480p destinations retain Main's porch-derived
placement.

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
