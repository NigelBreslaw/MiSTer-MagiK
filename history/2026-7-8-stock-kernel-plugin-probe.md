# Stock-Kernel Plugin Probe

Date: 2026-07-08

## Goal

Determine whether a loadable kernel module can provide fast writable back-buffer
mappings on the stock `5.15.1-MiSTer` kernel, without replacing the whole
kernel and without changing HDMI scanout.

## Implementation

- Added diagnostics-only module `mister_magik_plugin_probe.ko`.
- Built out-of-tree against `MiSTer-devel/Linux-Kernel_MiSTer` `MiSTer-v5.15`
  with `LOCALVERSION=-MiSTer`.
- Added MagiK diagnostics commands:
  - `plugin-map-report`
  - `plugin-map-bandwidth`
- Added one-shot runner `scripts/plugin-probe-one-shot.sh`.
- The runner checks module `vermagic`, loads with `insmod`, runs diagnostics,
  unloads with `rmmod`, restores the normal MagiK binary, and restarts the
  supervised launcher.

Module metadata:

```text
name:           mister_magik_plugin_probe
vermagic:       5.15.1-MiSTer SMP mod_unload ARMv7 p2v8
sha256:         d19f92fda8fc35018e9877b0d956ffeac66254a21ee26b42a85781fb6a268dc0
```

## Results

The module loaded on the stock kernel and exposed `/dev/mister-magik-plugin-probe`.
The module was unloaded after the run.

Reported regions:

| Region | Physical | Mmap | Notes |
| --- | ---: | --- | --- |
| adjacent fb resource | `0x220fe200` | ok | physical range immediately after active RGB565 frame |
| hidden slot 1 | `0x227e9000` | ok | existing HPS framebuffer slot |
| hidden slot 2 | `0x22fd2000` | ok | existing HPS framebuffer slot |
| plugin-owned DMA | `0x00000000` | unavailable | rejected by stock kernel DMA mask setup |

Bandwidth, 120 full-frame 960x540 RGB565 writes:

| Case | Avg Wall | P95 Wall | P99 Wall | Max Wall | Avg Bandwidth |
| --- | ---: | ---: | ---: | ---: | ---: |
| `/dev/fb0` active | 1,089 us | 1,279 us | 1,321 us | 1,895 us | 907.79 MB/s |
| plugin adjacent fb resource | 1,101 us | 1,279 us | 1,324 us | 1,804 us | 897.32 MB/s |
| plugin hidden slot 1 | 1,087 us | 1,278 us | 1,325 us | 1,763 us | 908.88 MB/s |
| plugin hidden slot 2 | 1,099 us | 1,302 us | 1,320 us | 1,799 us | 899.04 MB/s |
| hidden slot 1 via `/dev/mem` | 9,823 us | 9,868 us | 10,492 us | 10,999 us | 100.65 MB/s |

## Findings

- A stock-kernel `.ko` plugin is viable for exposing fast write-combined mappings.
- The plugin mappings are in the same speed class as `/dev/fb0`, and far faster
  than direct `/dev/mem`.
- The plugin can coexist with built-in `MiSTer_fb`; it does not replace `/dev/fb0`.
- Plugin-owned DMA memory was not available through this simple misc-device
  probe. The kernel rejected the 32-bit DMA mask setup, so this route should not
  be assumed viable without deeper driver/device integration.
- The plugin still cannot, by itself, make HDMI scan out from the mapped buffer.
  It provides fast CPU-write access only.

An earlier buggy version called `dma_alloc_wc(NULL, ...)`, which oopsed during
module init and required a reboot to clear the half-loaded module. The final
module allocates only after misc-device registration and skips plugin-owned DMA
when the DMA mask is rejected.

## Conclusion

Pass for shipping-feasibility of a stock-kernel plugin experiment. A `.ko` can
load on the stock MiSTer kernel and expose fast writable framebuffer-style
mappings without replacing the kernel.

This does not yet prove tear-free double buffering. The next experiment must
answer whether Main or a small control path can flip scanout to one of these
plugin-exposed physical addresses during vblank.

## Cleanup

- Module unloaded.
- Normal non-diagnostics MagiK binary restored.
- Supervised launcher restarted.
- No persistent boot config or module autoloading was added.
