# Application scenarios and physical input qualification

Everyday application correctness, measurements and optional profiles use the
shared [2.0 Python framework](../magik/README.md). Real MagiK and Mini-MagiK use
the same service and result format.

```sh
scripts/magik check
scripts/magik check idle
scripts/magik check idle --profile
PYTHONPATH=magik/host uv run --project magik/host pytest magik/scenarios --magik-device -k journeys
PYTHONPATH=magik/host uv run --project magik/host pytest magik/scenarios --magik-device -k journeys --magik-profile
```

The journeys exercise Arcade selection and return, and change/restore Reduce
motion. Two unprofiled repetitions retain host-observed response times, including
RPC and accessibility polling; these are not device frame latency or an FPS
benchmark. The separate profile uses the same journeys and a device-clock
measurement window and a 15-second whole-sequence allowance. The ten-second
profile samples the journey; it need not include all actions and cleanup.
Default smoke remains unchanged and does not select them.
The existing idle and Mini motion measurements remain available.
