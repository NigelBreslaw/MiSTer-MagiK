# Human Turbo Pacing

## Context

The `human-turbo-hold` Arcade benchmark had no CPU-work misses, but still
showed low-work/high-wall frame misses:

```text
PERFTRAIN-PACING-AFTER-DIRECT
work_gt_16_7ms=0
wall_gt_20ms=8
wall_gt_33ms=0
p99_wall_us=18677
max_wall_us=22467
```

The worst rows were not catalog, media, preview apply, or status writes. The
strongest correlation was vsync phase/wait age, and a normal production binary
does not run this scenario because `human-turbo-hold` and trace output require
`--bench-tools`.

## Attempts

A broad adaptive wait-before-render path made the benchmark worse:

```text
PERFTRAIN-PHASE-ADAPT-BENCHTOOLS
wall_gt_20ms=16
wall_gt_33ms=1
work_gt_16_7ms=1
max_wall_us=49832
```

That path was dropped. The narrower fix kept render-then-vsync by default and
only changed pacing in the observed bad cases:

- apply a foreground `launcher-ui` runtime thread policy;
- wait before render only when the loop starts in the final 6 ms of the current
  predicted vsync period;
- for direct `FBIO_WAITFORVSYNC`, sleep in userspace until the last 8 ms of the
  predicted period before arming the ioctl.

## Results

All rows below are real-device 30 second `human-turbo-hold` runs with a
bench-tools binary:

```text
PERFTRAIN-PACING-AFTER-DIRECT  wall_gt_20ms=8  p99_wall_us=18677  max_wall_us=22467  p99_work_us=8636
PERFTRAIN-LATEPHASE-AFTER      wall_gt_20ms=6  p99_wall_us=17796  max_wall_us=25957  p99_work_us=5915
PERFTRAIN-NICE10-AFTER         wall_gt_20ms=2  p99_wall_us=17476  max_wall_us=23296  p99_work_us=5560
PERFTRAIN-PREARM-AFTER         wall_gt_20ms=0  p99_wall_us=17408  max_wall_us=19896  p99_work_us=5519
```

`PERFTRAIN-PREARM-AFTER` kept clean vsync accounting:

```text
vsync=1800 fallback=0 timeout=0 error=0 max_miss_streak=0
work_gt_16_7ms=0 wall_gt_20ms=0 wall_gt_33ms=0
```

The profile script still returned non-zero because its stricter pacing gate also
requires `p99_wall_us < 16000` and `max_wall_us < 16667`. This fix removes the
reported `>20ms` frame misses, but the stricter near-drop gate remains a useful
target for future work.
