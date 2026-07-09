# 2026-07-08 Plugin Main-Present Experiment

## Summary

This experiment tested whether a stock-kernel loadable plugin can expose fast writable
launcher back buffers and whether the current Main-owned vblank present prototype can
drive the full launcher UI through them.

Result:

- **Fast plugin mappings are viable.** Hidden framebuffer slots exposed by the plugin
  write at `/dev/fb0`-class speed, roughly 1.1-1.2 ms per 960x540 RGB565 frame.
- **Main can route plugin-written hidden slots.** The pattern diagnostic successfully
  alternated hidden slots 1 and 2 through Main present requests.
- **The synchronous full-UI present path is not viable.** Full launcher scroll spent
  about 16 ms in `fb_present_us` and every measured frame missed 16.7 ms. The blocking
  request/ack path folds the vblank wait into the UI frame.

Recommendation: keep the plugin mapping route as useful evidence, but do not ship or
continue the synchronous launcher backend. The next plugin slice should prototype a
non-blocking present protocol, or stop and move the vblank wait/flip ownership out of
the UI thread.

## Commits In This Slice

- `245c303e` Add plugin present pattern diagnostic
- `e7c58a4b` Add plugin full-ui experiment runner
- `ced0d253` Tolerate transient Main present ack reads
- `4f714ac2` Widen Main present ack timeout
- `0b0c7f86` Keep plugin loaded during full-ui profile
- `b369e503` Flush launcher trace incrementally
- `21743f92` Build bench plugin probe binary for full UI runs
- `2f860770` Parse plugin present fields in arcade trace analyzer

Earlier commits in this branch introduced the plugin-backed hidden present mapping and
the stock-kernel plugin probe.

## Artifacts

- Plugin map report:
  `build/plugin-probe/plugin-map-report.log`
- Plugin bandwidth report:
  `build/plugin-probe/plugin-map-bandwidth.log`
- Plugin pattern present report:
  `build/plugin-probe/plugin-present-pattern.log`
- Full UI trace:
  `build/arcade-scroll-profiles/PLUGIN-MAIN-VSYNC-20260708T181140Z-arcade-scroll.tsv`
- Full UI log:
  `build/arcade-scroll-profiles/PLUGIN-MAIN-VSYNC-20260708T181140Z-arcade-scroll.log`
- Full UI status:
  `build/arcade-scroll-profiles/PLUGIN-MAIN-VSYNC-20260708T181140Z-arcade-scroll.status.json`

## Device And Safety State

- Live kernel was verified as `5.15.1-MiSTer`.
- Probe module was loaded with `insmod` only for the experiment.
- After the run, the module was unloaded and `/dev/mister-magik-plugin-probe` was gone.
- Normal production MagiK binary was redeployed.
- Normal launcher was running under `MiSTer_MagiK`.
- No stale reboot/fault arming files were present:
  `/media/fat/mister-magik/launcher.env`,
  `/tmp/mister-magik/fs-fault*`,
  `/media/fat/mister-magik/rebuild-on-next-boot`.

## Mapping Report

`/dev/fb0` remains a single visible 960x540 RGB565 mapping:

```text
smem_start=0x22001000
smem_len=1036800
line_length=1920
xres=960
yres=540
xres_virtual=960
yres_virtual=540
bpp=16
```

The plugin exposed these WC mappings:

```text
adjacent-fb-resource phys=0x220fe200 len=1040384
hidden-slot-1        phys=0x227e9000 len=1040384
hidden-slot-2        phys=0x22fd2000 len=1040384
plugin-owned-dma     unavailable
```

The plugin-owned DMA strategy still failed in this simple misc-device probe, so this
slice did not prove plugin-owned scanout memory. It did prove that a plugin can expose
existing framebuffer-related regions with fast CPU write attributes.

## Bandwidth Results

Each case copied 120 full 960x540 RGB565 frames, 1,036,800 bytes per frame.

| Case | Avg wall | P50 | P95 | P99 | Max | Avg MB/s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `/dev/fb0` active | 1122 us | 1089 us | 1329 us | 1415 us | 1711 us | 881.08 |
| plugin adjacent | 1135 us | 1104 us | 1330 us | 1400 us | 1611 us | 870.77 |
| plugin hidden slot 1 | 1176 us | 1151 us | 1274 us | 1647 us | 2309 us | 840.58 |
| plugin hidden slot 2 | 1190 us | 1174 us | 1284 us | 1339 us | 1788 us | 830.53 |
| hidden `/dev/mem` buffer 1 | 9915 us | 9905 us | 9954 us | 10038 us | 10566 us | 99.72 |

Decision: plugin WC hidden mappings meet the target. The old `/dev/mem` hidden-buffer
path remains ruled out.

## Pattern Present Results

The pattern diagnostic alternated hidden buffers 1 and 2 for 30 frames.

```text
copy_p50_us=1834
copy_p95_us=2299
copy_p99_us=2949
request_p50_us=20730
request_p95_us=21038
request_p99_us=21215
wait_p50_us=11468
wait_p95_us=11508
wait_p99_us=11515
route_p50_us=39
route_p95_us=188
route_p99_us=853
```

Decision: Main can route frames written through the plugin mapping, but the current
request path blocks for roughly one vblank plus polling/IPC overhead.

## Full UI Results

Command path:

```text
MISTER_PRESENT_BACKEND=plugin-main-vsync-hidden
scripts/profile-arcade-scroll.sh PLUGIN-MAIN-VSYNC-20260708T181140Z \
  --skip-build --secs 10 --scenario turbo-hold --skip-boot-prelude \
  --catalog-refresh off --stream-consumer none
```

The analyzer parsed 300 rows. Every frame exceeded 16.7 ms.

| Field | P50 | P95 | P99 | Max | Avg |
| --- | ---: | ---: | ---: | ---: | ---: |
| `main_present_hidden_copy_us` | 1438 | 1577 | 1915 | 2342 | 1464.2 |
| `main_present_request_us` | 14733 | 15545 | 16249 | 16303 | 14692.5 |
| `main_present_wait_us` | 11235 | 11309 | 11342 | 11362 | 10972.0 |
| `main_present_route_us` | 39 | 42 | 89 | 132 | 41.0 |
| `fb_present_us` | 16202 | 17048 | 17598 | 17671 | 16169.9 |
| `wall_us` | 33300 | 33448 | 33737 | 50704 | 33022.7 |

Frame budget:

```text
frames=300
wall_gt_16_7ms=300
wall_gt_33_3ms=40
```

Decision: this is not a CPU-copy problem anymore. The plugin copy is fast enough. The
UI thread is blocked by the synchronous Main present request, so it effectively waits
inside present and then starts the next frame badly phased.

## Interpretation

The plugin experiment changed the answer from "can a plugin write fast enough?" to
"yes, but the present ownership is still in the wrong place."

What worked:

- Stock-kernel `.ko` loads and unloads.
- Plugin WC mapping is fast enough.
- Hidden slots can be written by userspace through the plugin.
- Main can route those hidden-slot physical addresses to scanout.

What failed:

- Plugin-owned DMA allocation was unavailable through this simple probe.
- Synchronous request/ack makes full UI frame time roughly 33 ms.
- The launcher backend cannot block on Main's vblank wait and still hit 60 Hz.

## Next Options

1. **Async plugin/Main present queue.**
   Rust submits a present request and does not wait in the UI frame. It polls acks later.
   With only two hidden buffers, this may need frame dropping or a third buffer to avoid
   writing into an active/in-flight buffer.

2. **Main-owned paced compositor handoff.**
   Rust writes frames to plugin buffers and Main owns the vblank wait/flip loop. Rust
   never waits for vblank; it only publishes the newest complete buffer. This is likely
   the cleanest model if we keep the plugin route.

3. **Kernel/fb driver change.**
   If the project wants a production-quality double buffer, exposing official
   `/dev/fb0` virtual memory or a dedicated MagiK fb helper remains cleaner than a
   sidecar plugin that knows physical slots.

Do not continue optimizing the synchronous `plugin-main-vsync-hidden` launcher backend.
Its mapping is fast, but its scheduling model is wrong.
