# 2026-07-08 Fast back-buffer feasibility

## Setup

- Device: MiSTer at `192.168.1.117`.
- Kernel: `Linux MiSTer 5.15.1-MiSTer #1 SMP Wed Apr 2 20:01:54 CST 2025 armv7l`.
- Driver: built-in `MiSTer_fb`, `drivers/video/fbdev/MiSTer_fb`.
- MagiK diagnostics commands:
  - `fb-map-report`
  - `fb-map-bandwidth 120`

## Artifacts

- Mapping report: `build/fb-map-report.log`.
- Bandwidth report: `build/fb-map-bandwidth.log`.

## Mapping report

`FBIOGET_FSCREENINFO`:

```text
id=MiSTer_fb
smem_start=0x22001000
smem_len=1036800
line_length=1920
type=0
visual=2
xpanstep=0
ypanstep=0
ywrapstep=0
capabilities=0x0
```

`FBIOGET_VSCREENINFO`:

```text
xres=960
yres=540
xres_virtual=960
yres_virtual=540
xoffset=0
yoffset=0
bpp=16
red=11:5:0
green=5:6:0
blue=0:5:0
```

MagiK expected RGB565 geometry:

```text
width=960
height=540
stride_bytes=1920
frame_bytes=1036800
double_bytes=2073600
hidden_slot_bytes=8294400
```

Mmap probes:

```text
active_frame requested_len=1036800 ok=1
two_rgb565_frames requested_len=2073600 ok=0 error=Invalid argument (os error 22)
reported_smem_len requested_len=1036800 ok=1
```

Sysfs agreed with the ioctl values:

```text
/sys/module/MiSTer_fb/parameters/mode=565 1 960 540 1920
/sys/module/MiSTer_fb/parameters/width=960
/sys/module/MiSTer_fb/parameters/height=540
/sys/module/MiSTer_fb/parameters/stride=1920
/sys/class/graphics/fb0/virtual_size=960,540
/sys/class/graphics/fb0/stride=1920
/sys/class/graphics/fb0/bits_per_pixel=16
```

## Bandwidth report

Frame size: 960x540 RGB565, 1,036,800 bytes.

| Target | Valid | Avg wall | P50 | P95 | P99 | Max | Avg throughput |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `/dev/fb0` active mapping | yes | 1,581us | 1,589us | 1,698us | 1,744us | 2,239us | 625.22 MB/s |
| `/dev/fb0` second-frame range | no | - | - | - | - | - | skipped: `smem_len` is only one frame |
| hidden buffer 1 via `/dev/mem` | yes | 11,316us | 11,313us | 11,425us | 11,545us | 11,997us | 87.38 MB/s |

This is consistent with the earlier hidden-copy result:

```text
hidden /dev/mem copy avg ~= 10.3ms
hidden /dev/mem copy p99 ~= 12.1ms
```

## Driver/source finding

The live driver is built into the kernel rather than loaded as a `.ko`:

```text
name: MiSTer_fb
filename: (builtin)
file: drivers/video/fbdev/MiSTer_fb
author: Sorgelig@MiSTer
description: MiSTer framebuffer driver
parameters: width height stride format rb frame_count res_count
```

The local repository/reference check did not include the kernel driver source,
so this slice relies on deployed driver metadata plus runtime ioctl/sysfs
evidence. That evidence is enough for the immediate decision: the current
`MiSTer_fb` export deliberately presents one active framebuffer region to Linux.

## Conclusion

Current `/dev/fb0` exposes exactly one fast RGB565 frame. It does not expose a
second writable frame, and mapping two 960x540 RGB565 frames through `/dev/fb0`
fails with `EINVAL`.

The only currently accessible hidden buffers are the Main/FPGA physical slots
through `/dev/mem`, and those are far too slow for launcher presentation.

## Recommendation

The viable next implementation path is not a Main-only change. It requires a
driver/kernel-side mechanism to expose at least one extra framebuffer region with
the same fast mapping attributes as `/dev/fb0`.

Recommended next prototype:

1. Locate or fetch the exact `MiSTer_fb` driver source matching the deployed
   kernel.
2. Add a minimal experimental driver branch that exposes two RGB565 frames, or a
   second mmap-able framebuffer device, while preserving the current mode/sysfs
   contract.
3. Re-run `fb-map-report` and `fb-map-bandwidth`.
4. Only if the second mapping reaches `/dev/fb0`-class throughput should we
   resume Main-owned vblank flip work.

Do not continue with `/dev/mem` hidden buffers or an async mailbox until there is
a fast writable back buffer.

## Cleanup

- Restored normal non-diagnostics MagiK binary.
- Cleared `/media/fat/mister-magik/launcher.env`.
- Restarted the normal launcher through Main.
- Removed stale volatile present ack file from `/tmp/mister-magik`.
