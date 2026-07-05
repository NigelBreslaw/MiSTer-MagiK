# Framebuffer Stream V1 Benchmark

Date: 2026-07-05
Device: reference MiSTer at `192.168.1.117`

## Build

- Agent: `mister-magik-agent` with `framebuffer_stream_v1`.
- Framebuffer app: `release-device`, then `release-device --all-scenes --bench-tools`
  for the active launcher scenario.
- Desktop benchmark entrypoint:
  `cargo run --manifest-path desktop/Cargo.toml --locked -- --framebuffer-{poll,stream}-bench 120`

## Rows

```text
framebuffer_stream_bench_tsv	mode=poll	frames=120	fps=10.43	elapsed_ms=11509	p50_ms=81.0	p95_ms=164.6	avg_payload_bytes=11471	avg_raw_bytes=1036800
framebuffer_stream_bench_tsv	mode=stream	frames=1	fps=8.49	elapsed_ms=118	p50_ms=90.6	p95_ms=90.6	avg_payload_bytes=11461	avg_raw_bytes=1036800
framebuffer_stream_bench_tsv	mode=stream	frames=120	fps=21.24	elapsed_ms=5650	p50_ms=42.6	p95_ms=45.1	avg_payload_bytes=10808	avg_raw_bytes=779886
framebuffer_stream_bench_tsv	mode=poll	frames=120	fps=6.38	elapsed_ms=18816	p50_ms=148.0	p95_ms=232.9	avg_payload_bytes=11002	avg_raw_bytes=1036800
```

## Notes

- The idle polling row reproduced the previous baseline almost exactly:
  `10.43 fps` vs the earlier `10.46 fps`.
- The sustained stream row used the real launcher present path with
  `MISTER_LAUNCHER_BENCH_SCENARIO=home-repeat-hold`.
- Producer stream reached `21.24 fps`, just over 2x the reproduced polling
  baseline and 3.33x the same-scenario polling row.
- The first active-scene attempt used `tear_pattern`, but that scene bypasses
  the tapped `UiFrameTarget` path. A direct `nc` probe saw no producer bytes
  there. The launcher scenario did emit valid `MMFSv1` frames.
- Current v1 still compresses and writes on the UI present path. During a direct
  `nc` probe, launcher `fb-present` rose sharply, so the next large win should
  move compression/socket writes to a producer worker with a latest-frame queue.

