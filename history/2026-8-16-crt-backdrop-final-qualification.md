# CRT backdrop cleanup qualification — 2026-08-16

This record covers the cleanup commits added after the 172-commit CRT UI
development range. The installed Dev runtime was built from
`a51e1b5b3` (`refactor(agent): consolidate CRT scroll profiling`) and passed
the repository delivery workflow. The platform bundle was the reused
`platform-v0.28` bundle with the installed GUI source revision
`a51e1b5b3fb526da848837ea7ee167f1832a0aba`.

## Control qualification

The canonical `arcade-velocity-scroll` control was run for 40 seconds on both
supported CRT routes. Both legs passed the fixed v1 quality contract.

| Route | Evidence | Frames | Physical refresh | Dropped frames | Sequence gaps | Minimum FPS | Backdrop prepare p95 / max | Blend p95 / max |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `crt-240p60` | `build/agent-benchmarks/arcade-velocity-scroll/1786876119` | 2459 | 60.053 Hz | 0 | 0 | 59.9 | 4,771 / 11,838 us | 1,137 / 1,503 us |
| `crt-288p50` | `build/agent-benchmarks/arcade-velocity-scroll/1786876412` | 2067 | 50.429 Hz | 0 | 0 | 49.9 | 5,031 / 11,279 us | 0 / 1,316 us |

Each leg also reported zero latch-drop delta, zero repeated-vblank frames,
zero ownership losses, varied authoritative RGB565 scanout content, and
`quality_status=passed`. The authoritative artifacts are the per-leg
`summary.json`, `profile.json`, `telemetry.jsonl`, and `terminal-arcade.png`
under the evidence directory above.

## Attribution qualification

The public profiler surface is now the single
`arcade-velocity-scroll-attribution` scenario. It was attempted on both CRT
routes after the control legs. Neither attempt produced a valid artifact:

- On `crt-240p60`, the first attempt stopped in the pprof arm with
  `Resource temporarily unavailable (os error 11)`; the bounded retry then
  stopped in GUI automation cleanup with `No such file or directory (os error 2)`.
- On `crt-288p50`, the attempt stopped in the PMU arm with GUI automation
  cleanup reporting `No such file or directory (os error 2)`.

These are device automation failures before attribution evidence completion,
not quality passes. The typed checks after the failures reported
`arming=clear` and a healthy latch. No attribution result is claimed here.

## Restoration

The original route was restored with the typed attended command
`scripts/agent device display set crt240p60 --attended --keep`. Final typed
checks reported `arming=clear`, `active_width=640`, `active_height=240`, and
`drop_count=0`.
