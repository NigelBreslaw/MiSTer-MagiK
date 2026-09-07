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

## Inspection

Use `scripts/magik device display status` or an explicit `display set MODE
--attended`. Framebuffer capture uses `scripts/magik mcp`: raw pixels are
unchanged; its optional display view applies the documented 4:3 scanline mapping.
See [framebuffer capture](../magik/docs/framebuffer-capture.md).

Focused automated assertions belong in Python scenarios. Validate physical
output on the connected display when the claim concerns its actual raster.
