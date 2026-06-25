# LaunchHandoffSession Metrics

Parent: `ee4557fd8665045adeb26fb703664febe37be851`

## Benchmarks

- BEFORE handoff: `scripts/profile-launch-handoff.sh ARCH-HANDOFF-BEFORE --replace-label --iterations 5`
- AFTER handoff: `scripts/profile-launch-handoff.sh ARCH-HANDOFF-AFTER --replace-label --iterations 5`
- BEFORE prep: `scripts/profile-launch-prep.sh ARCH-HANDOFF-BEFORE --replace-label --iterations 10`
- AFTER prep: `scripts/profile-launch-prep.sh ARCH-HANDOFF-AFTER --replace-label --iterations 10`

## Handoff Summary

- `launch_action_to_loading_us`: BEFORE min=31926 median=33013 max=43059 mean=35964.8; AFTER min=32802 median=35600 max=45700 mean=37157.2.
- `max_frame_gap_us`: BEFORE min=16748 median=20447 max=29366 mean=20838.8; AFTER min=17903 median=17983 max=21435 mean=19183.4.
- `failure_recovery_us`: BEFORE min=1354 median=1373 max=1679 mean=1437.8; AFTER min=1351 median=1379 max=1419 mean=1381.0.
- `loading_frames_before_result`: BEFORE 47-48; AFTER 47-48.

`launch_action_to_loading_us` remains within the observed run noise. `max_frame_gap_us` and `failure_recovery_us` improve.

## Launch Prep Summary

- BEFORE summary: `errors=0 count=120 p50_us=101 p95_us=2986 write_bytes=163840 wchar=1080`.
- AFTER summary: `errors=0 count=120 p50_us=91 p95_us=2895 write_bytes=163840 wchar=1080`.

Named launch-prep p95 improves from `2986us` to `2895us`.

## Evidence

- `history/toolchain-bench/results-launch-handoff.tsv`
- `history/toolchain-bench/results-launch-prep.tsv`
- `build/launch-handoff/ARCH-HANDOFF-BEFORE.tsv`
- `build/launch-handoff/ARCH-HANDOFF-BEFORE.log`
- `build/launch-handoff/ARCH-HANDOFF-AFTER.tsv`
- `build/launch-handoff/ARCH-HANDOFF-AFTER.log`

