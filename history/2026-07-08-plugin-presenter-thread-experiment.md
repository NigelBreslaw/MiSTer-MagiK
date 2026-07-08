# 2026-07-08 - Experiment A: Kernel Plugin Presenter Thread

## Summary

Experiment A tested whether the stock-kernel plugin can become the asynchronous
presenter: MagiK would copy into plugin-exposed fast framebuffer memory, post a
present request to a kernel module, and return to UI work while a kernel thread
waited for vblank and flipped the active buffer.

Result: the stock-kernel plugin route is useful for fast mappings, but it is not
yet a presenter. The module can accept and record async-present-shaped requests,
but it reports no supported vblank/route ownership path:

```text
plugin_presenter_capability_tsv supported=0 reason=no-uio-route-symbol vblank_owner=unsupported route_owner=unsupported
```

No full UI backend was wired on this path. That was intentional: without a
kernel-side flip mechanism, a `plugin-kthread-hidden` launcher backend would only
queue requests that cannot be presented.

## Commits

- `f1d78a9f Add plugin presenter mailbox diagnostic`
- `d24daf24 Allow plugin probe runner to skip full UI path`
- `8c420c62 Disable stack protector for plugin probe module`
- `68541e4b Parse plugin presenter mailbox fields`

## Device Procedure

Command:

```bash
MISTER_PLUGIN_SKIP_FULL_UI=1 MISTER_PLUGIN_PRESENT_PATTERN_FRAMES=10 MISTER_PLUGIN_PROBE_FRAMES=60 scripts/plugin-probe-one-shot.sh
```

The runner:

- built `mister_magik_plugin_probe.ko` against `5.15.1-MiSTer`;
- built and deployed a diagnostics-enabled MagiK binary;
- loaded the module once with `insmod`;
- ran `plugin-map-report`, `plugin-presenter-report`,
  `plugin-map-bandwidth`, and a short synchronous `plugin-present-pattern`;
- unloaded the module;
- restored the normal non-diagnostics MagiK binary and restarted the
  Main-supervised launcher.

## Presenter Mailbox Evidence

The mailbox post itself works and is cheap:

```text
plugin_presenter_post_tsv ok=1 post_us=61 request=plugin_present_async_v1 sequence=1 buffer=1 width=960 height=540 stride=1920
```

The module recorded the request:

```text
plugin_presenter_after_plugin_presenter_status_tsv posted_sequence=1 posted_buffer=1 posted_width=960 posted_height=540 posted_stride=1920 pending_sequence=1 post_count=1 reject_count=0 last_error=kernel-presenter-unsupported:no-uio-route-symbol
```

But no flip occurred:

```text
active_sequence=0 active_buffer=0 flip_count=0
```

Interpretation: a misc-device plugin can provide a mailbox, but the current
stock-kernel module has no exported route to Main's `UIO_WAIT_VSYNC` /
`UIO_SET_FBUF` path and no implemented direct FPGA-register presenter.

## Mapping Bandwidth Evidence

All measured plugin write-combined mappings remain `/dev/fb0`-class fast:

| Case | avg wall | p99 wall | avg bandwidth |
| --- | ---: | ---: | ---: |
| `/dev/fb0` active | 1197 us | 1767 us | 825.78 MB/s |
| plugin adjacent fb resource | 1196 us | 1720 us | 826.15 MB/s |
| plugin hidden slot 1 | 1195 us | 1703 us | 827.17 MB/s |
| plugin hidden slot 2 | 1184 us | 1697 us | 834.42 MB/s |
| hidden `/dev/mem` buffer 1 | 9920 us | 10749 us | 99.66 MB/s |

This confirms the performance problem is not bulk writes when the module maps
the memory with the right attributes. The remaining blocker is the authority and
mechanism to make HDMI scan out the posted buffer without blocking MagiK.

## Synchronous Pattern Comparison

The existing Main-mediated diagnostic still blocks on present:

```text
plugin_present_pattern_summary_tsv frames=10 copy_p50_us=1867 copy_p99_us=2403 request_p50_us=20744 request_p99_us=21661 wait_p50_us=11323 wait_p99_us=12880 route_p50_us=40 route_p99_us=959
```

Copy cost is acceptable here. The visible problem remains that the caller is
waiting around a whole vblank cycle, so this is not a fix for launcher frame
pacing.

## Issues Found And Fixed

The first device attempt failed to load the module:

```text
mister_magik_plugin_probe: Unknown symbol __stack_chk_guard
```

The probe module now builds with `-fno-stack-protector`.

The next attempt exposed a mailbox parser bug: the kernel parser passed the
whole trailing field string to `kstrtouint`, causing `EINVAL` for
`sequence=1 buffer=...`. The parser now copies only the token value before
conversion.

## Conclusion

Experiment A should stop at this feasibility boundary.

What worked:

- stock `5.15.1-MiSTer` accepts the loadable diagnostics module;
- plugin WC mappings are fast enough for full-frame RGB565 copies;
- a misc-device mailbox can accept async-present-shaped requests with tiny
  userspace overhead.

What did not work:

- the plugin cannot currently wait for vblank and flip through the existing
  Main route;
- no stock-kernel symbol/API gives the module ownership of `UIO_WAIT_VSYNC` and
  `UIO_SET_FBUF`;
- plugin-owned DMA memory is still unavailable in this simple probe.

## Recommendation

Do not implement the full `plugin-kthread-hidden` UI backend on this foundation
yet. It would have fast writes and fast request posting, but no proven presenter.

Next best experiments:

1. FPGA-side latch/register experiment: add a tiny MagiK-owned present latch in
   the video path and have the plugin or Main write pending buffer metadata,
   with the FPGA applying it at vblank.
2. Direct FPGA register plugin experiment: inspect the video register map and
   test whether a module can safely reproduce the minimal `UIO_WAIT_VSYNC` /
   `UIO_SET_FBUF` sequence without Main's helper symbols.
3. Tiny kernel/fb driver patch: expose the second fast page through the existing
   framebuffer driver and, if needed, add a minimal supported present ioctl.

The current evidence argues for option 1 first if we want a clever route that
uses the FPGA core instead of trying to smuggle Main's presentation semantics
through a stock-kernel misc device.

