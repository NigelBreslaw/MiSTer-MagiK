# Tiny Kernel Experiment: Fast Second `/dev/fb0` Mapping

Date: 2026-07-08

## Goal

Determine whether the MiSTer framebuffer driver can expose a second writable
960x540 RGB565 buffer through `/dev/fb0` with the same write speed as the
current active mapping. This experiment intentionally did not add Main flipping,
async present, launcher wiring, or a production hidden-buffer path.

## Source Inspection

Kernel source: `MiSTer-devel/Linux-Kernel_MiSTer`, branch `MiSTer-v5.15`,
revision `97a398176b362d6ba5f2db298c673fee1ffc0234`.

Relevant driver: `drivers/video/fbdev/MiSTer_fb.c`.

Findings:

- The deployed mode string `565 1 960 540 1920` is `format rb width height stride`.
  The second field is red/blue routing, not framebuffer count.
- The existing `frame_count` parameter is an IRQ/vsync counter, not a buffer
  count.
- The driver resource comes from the DE10 Nano DTS:
  `reg = <0x22000000 0x800000>`.
- The driver maps that resource with `memremap(..., MEMREMAP_WT)`.
- `/dev/fb0` begins at `fb_res->start + 4096`, leaving the first 4 KiB as a
  control/padding area. For 960x540 RGB565, two frames require 2,073,600 bytes,
  which fits inside the 8 MiB framebuffer-owned resource.

## Patch

Summary:

- Added experimental writable parameter `magik_mmap_frames`, default `1`.
- Kept production default behavior unchanged.
- Computed exported frame count from `magik_mmap_frames`, clamped to the
  framebuffer resource size after the first 4 KiB.
- Set `yres_virtual = yres * exported_frames`.
- Set `smem_len = line_length * yres_virtual`.
- Preserved the existing framebuffer mmap path and mapping attributes.

Artifact hashes:

```text
c50d33a2e19e3dd5151f030502878d831d1ebe1233c17b5869a60ddebb143021  zImage-magik-mmap-frames
1e9655be4a7eb48d87030467810d77277b645d5fcb1c0118d0650caa2a074d1a  socfpga_cyclone5_de10_nano-magik-mmap-frames.dtb
28d81eebb6d83ba9ec3849ef577fdaa09990bca5eb75d1f45282f9323f4ec399  magik-mmap-frames.patch
2338d598dc0db5b667daa03b39d792afc86776a8919a9b40e18eb444e93b6c57  zImage_dtb-magik-mmap-frames
```

The generated patch hash was
`28d81eebb6d83ba9ec3849ef577fdaa09990bca5eb75d1f45282f9323f4ec399`.

Device stock boot artifact before and after restore:

```text
a6c7b1be0da9ba24a91bc1816737915d6a6cfba27c6c3025caded95167dc8dae  /media/fat/linux/zImage_dtb
```

## Raw Device Results

Experimental kernel booted as:

```text
Linux MiSTer 5.15.1+ #1 SMP Wed Jul 8 16:39:47 UTC 2026 armv7l GNU/Linux
```

One-frame default after `magik_mmap_frames=1`:

```text
fb_map_report_fix_tsv	id=MiSTer_fb	smem_start=0x22001000	smem_len=1036800	line_length=1920
fb_map_report_var_tsv	xres=960	yres=540	xres_virtual=960	yres_virtual=540	bpp=16
fb_map_report_expected_tsv	width=960	height=540	stride_bytes=1920	frame_bytes=1036800	double_bytes=2073600	hidden_slot_bytes=8294400
fb_map_report_mmap_tsv	label=active_frame	requested_len=1036800	ok=1
fb_map_report_mmap_tsv	label=two_rgb565_frames	requested_len=2073600	ok=0	error=Invalid argument (os error 22)
fb_map_report_mmap_tsv	label=reported_smem_len	requested_len=1036800	ok=1
```

Two-frame experiment after `magik_mmap_frames=2`:

```text
fb_map_report_fix_tsv	id=MiSTer_fb	smem_start=0x22001000	smem_len=2073600	line_length=1920
fb_map_report_var_tsv	xres=960	yres=540	xres_virtual=960	yres_virtual=1080	bpp=16
fb_map_report_expected_tsv	width=960	height=540	stride_bytes=1920	frame_bytes=1036800	double_bytes=2073600	hidden_slot_bytes=8294400
fb_map_report_mmap_tsv	label=active_frame	requested_len=1036800	ok=1
fb_map_report_mmap_tsv	label=two_rgb565_frames	requested_len=2073600	ok=1
fb_map_report_mmap_tsv	label=reported_smem_len	requested_len=2073600	ok=1
```

Bandwidth with 120 full-frame RGB565 samples:

| Case | Valid | Avg Wall | P95 Wall | P99 Wall | Max Wall | Avg Bandwidth |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| `/dev/fb0` active frame | yes | 1,184 us | 1,268 us | 1,446 us | 1,723 us | 834.67 MB/s |
| `/dev/fb0` second-frame range | yes | 1,180 us | 1,240 us | 1,288 us | 1,779 us | 837.46 MB/s |
| hidden `/dev/mem` buffer 1 | yes | 10,051 us | 10,093 us | 10,191 us | 10,612 us | 98.38 MB/s |

## Restore

The device was restored to the stock kernel after the experiment:

```text
Linux MiSTer 5.15.1-MiSTer #1 SMP Wed Apr 2 20:01:54 CST 2025 armv7l GNU/Linux
a6c7b1be0da9ba24a91bc1816737915d6a6cfba27c6c3025caded95167dc8dae  /media/fat/linux/zImage_dtb
```

The normal non-diagnostics MagiK binary was rebuilt and deployed. Main resumed
the launcher successfully. No stale fault or launcher environment files were
present in the final hygiene check.

## Conclusion

Pass. A tiny `MiSTer_fb` driver change can expose a second `/dev/fb0` range with
the same write-combined-class performance as the active frame. The second range
is roughly 8.5x faster than the hidden `/dev/mem` path and meets the target
`p99 < 3 ms` full-frame write time.

This proves mapping feasibility only. It does not yet prove tear-free vblank
presentation, because no Main flip or HDMI scanout change was part of this
experiment.

## Recommendation

Next step: turn this into a small maintained kernel/driver branch and connect it
to the existing Main-owned vblank flip prototype. Main still needs a safe
no-copy present path that points scanout at the selected frame during vblank,
while Rust writes the other frame. If that cannot be expressed through the
existing Main/FPGA buffer select path, the next smallest driver change is a
dedicated explicit flip/pan/control path for these two `/dev/fb0`-mapped pages.
