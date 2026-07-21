# Native CRT output

MiSTer MagiK's first native CRT mode is a conservative progressive
640x240 RGB565 route. The existing HDMI/scaler route remains the default and
the stock root `menu.rbf` remains untouched.

## Source and license provenance

The implementation is derived only from GPL-licensed MiSTer project sources:

- `MiSTer-devel/Template_MiSTer` defines the core video and DDR interfaces.
- `MiSTer-devel/Menu_MiSTer` defines the Menu integration, video routing, and
  framework behavior.
- `MiSTer-devel/Arcade-BlackWidow_MiSTer` supplies reviewed design patterns for
  bounded DDR bursts, line-buffer ownership, safe reset draining, and display
  promotion at vertical blank.

No Zaparoo source, timing table, protocol, or memory layout is an input to this
design. Adapted RTL must retain an attribution comment identifying the official
MiSTer source that informed it.

## Version 1 timing

The native route uses a 25.175 MHz video clock and advances the raster on every
second clock edge. Its effective pixel rate is 12.5875 MHz.

| Region | Horizontal | Vertical |
| --- | ---: | ---: |
| Active | 640 | 240 |
| Front porch | 16 | 3 |
| Negative sync | 96 | 3 |
| Back porch | 48 | 16 |
| Total | 800 | 262 |

The resulting nominal rates are 15.734375 kHz and 60.054866 Hz. RGB is black
outside the active rectangle. PAL, interlace, centering controls, and
alternative active widths are not part of version 1.

## Memory and reader contract

The CRT reader consumes the existing kernel scanout ABI v3 without changing
its allocation:

- format: RGB565;
- geometry: exactly 640x240;
- stride: exactly 1,280 bytes;
- valid bases: `0x227e9000` and `0x22fd2000` only; and
- promotion: a posted slot becomes visible only at CRT vertical blank.

Each line is 160 64-bit DDR beats and is requested as bursts of 128 and 32.
Two line buffers cross from the 100 MHz DDR/system clock to the video clock.
Malformed control data, a DDR timeout, or a line underrun produces black rather
than stale or uninitialized memory. Status retains evidence counters for
underruns and timeouts.

The stock Menu DDR-clearing client must not share the interface in the MagiK
RBF. The CRT reader is the sole Menu-core DDR client after integration.

## Latch protocol v3

Commands `0x57`, `0x58`, and `0x59` retain their discovery magic. Version 3
keeps the version-2 words in their existing positions and appends route data.

### Set command `0x57`

Words 0-9 retain the framebuffer route fields. Word 10 still commits the
sequence. Word 11 stages the requested output route for that commit:

- `0`: HDMI;
- `1`: CRT 640x240p60.

Unknown routes are rejected to HDMI. A pending HDMI request applies on the next
synchronized `hdmi_vbl`; a pending CRT request applies on the next CRT vertical
blank. The route and every framebuffer field change atomically at that
destination boundary.

### Status command `0x58`

Words 0-10 retain the version-2 layout. Appended words are:

| Word | Meaning |
| ---: | --- |
| 11 | requested route |
| 12 | active route |
| 13 | reader flags: valid, fallback, underrun, timeout |
| 14 | underrun count |
| 15 | DDR-timeout count |

### Capabilities command `0x59`

Words 0-4 retain maximum framebuffer geometry. Appended words are:

| Word | Meaning |
| ---: | --- |
| 5 | supported-route bitmap (`bit 0` HDMI, `bit 1` CRT 240p60) |
| 6 | native timing-table version (`1`) |

Version-2 clients read their original five words and continue to use HDMI.
Version-3 clients accept either protocol version, but may request CRT only
when protocol 3 and the CRT capability bit are both present. Missing or
malformed capability data always selects HDMI.

## Runtime ownership and safety

`direct_video=2` is resolved by the maintained Main fork using its known-DAC
EDID table. Main reports the resolved state (`hdmi` or `crt-240p60`) to the
launcher through the versioned runtime-settings boundary. Rust must not infer
the resolved route from the literal INI value.

The FPGA powers up on HDMI and remains there until a valid protocol-v3 request.
Main owns crash and lifecycle fallback. A frontend failure must leave a black
or HDMI-recoverable display and must never trigger a reboot. The attended CRT
trial is volatile, bounded to 30 seconds, and restores the prior route on
normal completion, error, or interruption.
