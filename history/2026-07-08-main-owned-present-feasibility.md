# 2026-07-08 Main-owned present feasibility slice

## Setup

- Device: MiSTer at `192.168.1.117`.
- Main fork: `MiSTer_MagiK` with volatile present request parsing and vblank ack path.
- MagiK prototype backend: `MISTER_PRESENT_BACKEND=main-vsync-hidden`.
- Test scenario: `MISTER_LAUNCHER_BENCH_SCENARIO=home-repeat-hold`.
- Trace window: 20 seconds, post-warmup frames after frame 30.

## Artifacts

- Experimental trace: `build/main-vsync-hidden-home-repeat.tsv`.
- Experimental log: `build/main-vsync-hidden-home-repeat.log`.
- Default `/dev/fb0` baseline trace: `build/fb0-home-repeat.tsv`.
- Default `/dev/fb0` baseline log: `build/fb0-home-repeat.log`.
- Hidden copy benchmark log: `build/hidden-fb-copy-bench.log`.

## Results

### Default `/dev/fb0` baseline

- Frames after warmup: 1166.
- Average FPS: 59.8 over the 20 second run.
- `wall_us`: avg 16,470; p50 16,636; p95 16,866; p99 16,892; max 17,843.
- `work_us`: p99 8,662.
- `fb_present_us`: avg 1,060; p95 1,417; p99 1,485; max 1,707.
- Vsync source: 1165 `vsync`, 0 fallback/timeout/error, max miss streak 0.
- Frame pacing gate failed strict wall thresholds because 221 frames were just over
  16,667us, but CPU work stayed well under budget.

### Experimental `main-vsync-hidden`

- Frames after warmup: 390.
- Average FPS: 21.0 over the 20 second run.
- `wall_us`: avg 47,938; p50 49,963; p95 50,820; p99 51,106; max 51,185.
- `work_us`: p99 35,573.
- `fb_present_us`: avg 26,968; p95 28,833; p99 28,922; max 29,002.
- `main_present_hidden_copy_us`: avg 7,717; p50 9,670; p95 9,693; p99 9,742; max 10,743.
- `main_present_request_us`: avg 13,544; p50 17,995; p95 18,317; p99 18,395; max 18,724.
- `main_present_wait_us`: avg 11,728; p50 15,741; p95 16,021; p99 16,070; max 16,085.
- `main_present_route_us`: avg 32; p50 39; p95 40; p99 90; max 93.
- Vsync source: 389 `vsync`, 0 fallback/timeout/error, max miss streak 0.
- Main present ack path was alive: latest observed ack was `status=ok`.
- Frame pacing gate failed hard: every post-warmup frame exceeded 16,667us.

### Raw hidden buffer copy benchmark

- Command: `hidden-fb-copy-bench 240`.
- Frame size: 960x540 RGB565, 1,036,800 bytes per frame.
- Total copied: 248,832,000 bytes.
- `wall_us`: avg 10,313; p50 10,281; p95 10,671; p99 12,056; max 13,003.
- `cpu_us`: avg 9,971; p50 9,960; p95 10,247; p99 10,460; max 11,366.
- Throughput: avg 95.96 MB/s; min 76.04 MB/s.

## Interpretation

The prototype proves Main can accept volatile present requests, wait for vblank,
route a hidden buffer with `UIO_SET_FBUF`, and ack the launcher. The actual route
operation is cheap: roughly 40us.

The current design is not performant because it pays two large costs per
presented frame:

1. Rust copies the full 960x540 RGB565 frame into hidden `/dev/mem` memory. That
   costs about 10ms by itself.
2. Rust then performs a blocking file request/ack round trip in which Main waits
   for the next vblank. In the measured loop this adds roughly another 16ms.

Those costs stack after rendering, so the prototype lands near 50ms/frame
instead of 16.7ms/frame.

## Conclusion

Main-owned no-copy scan-out switching is mechanically feasible, but this
user-space file-request plus `/dev/mem` full-frame copy prototype is not a viable
production present path.

The next viable direction is to keep the cheap part, Main's vblank `UIO_SET_FBUF`
route, while removing at least one of the expensive parts:

- avoid `/dev/mem` full-frame copies by getting a fast mapping for hidden buffers,
- or make Main drive/asynchronously consume pre-posted buffer state without a
  per-frame blocking request file,
- or move the double-buffer ownership lower, where buffer flips and producer
  writes can be coordinated without waking two user-space processes per frame.

## Cleanup

- Restored normal `ui` MagiK binary.
- Cleared `/media/fat/mister-magik/launcher.env`.
- Restarted normal launcher through Main.
- Verified no stale present request/ack or fault arming files were present.
