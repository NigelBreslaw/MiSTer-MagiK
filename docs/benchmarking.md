# Application scenarios and physical input qualification

Everyday application correctness, measurements and optional profiles use the
shared [2.0 Python framework](../magik2/README.md). Real MagiK and Mini-MagiK use
the same service and result format.

```sh
scripts/magik2 check
scripts/magik2 check idle
scripts/magik2 check idle --profile
PYTHONPATH=magik2/host uv run --project magik2/host pytest magik2/scenarios --magik2-device -k journeys
PYTHONPATH=magik2/host uv run --project magik2/host pytest magik2/scenarios --magik2-device -k journeys --magik2-profile
```

The journeys exercise Arcade selection and return, and change/restore Reduce
motion. Two unprofiled repetitions retain host-observed response times, including
RPC and accessibility polling; these are not device frame latency or an FPS
benchmark. The separate profile uses the same journeys and a device-clock
measurement window and a 15-second whole-sequence allowance. The ten-second
profile samples the journey; it need not include all actions and cleanup.
Default smoke remains unchanged and does not select them.
The existing idle and Mini motion measurements remain available.

## Retained physical input qualification

`scripts/agent benchmark input-integrity` is the only retained legacy benchmark.
It tests Main's real mapping, aggregation, proxy and kernel input path, which
Slint events cannot establish. See [Unified input](input.md). It preserves its
installed-platform identity checks and zero-loss evidence; it never builds or
deploys an application. It is explicitly requested, not a development prerequisite.

Main/core-return and FPGA qualification remain under their separate attended
release/platform workflows. They are not migrated into development scenarios.

The old application experiment registry, attribution campaigns and UI harness
are retired. Historical reports may describe their results; their commands are
no longer operational instructions. No legacy campaign has a compatibility alias.
