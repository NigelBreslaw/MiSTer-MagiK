# Slow Frame Attribution

Item: attribute rare frame spikes.

## Confirmed Cause

The preview-scroll benchmark could detect frames where UI work exceeded the
16.667 ms frame budget, but it only printed `work_us`, `wall_us`, and
`vsync_us`. The trace did not contain enough detail to tell whether the slow
work came from preview work, worker draining, media gate handling, status text,
or the dominant render phase.

## Fix

When preview-scroll tracing is enabled, the launcher now records prepare-phase
subtimings for catalog worker draining, media worker draining, media gate/seed
handling, preview scheduling, preview apply, and status string copies. Runtime
status write time is measured in `finish_frame` and appended to the same trace
row.

`scripts/profile-preview-scroll.sh` now emits:

- `slow_frame_attribution_tsv` for each `work_us > 16667` frame.
- `slow_frame_attribution_summary_tsv` with attributed/unattributed counts.
- `metric_tsv` for `unattributed_slow_work_frames`.

Specific subsystem labels are only used when the subsystem is at least 1 ms and
at least half of the dominant phase; otherwise the row falls back to
`dominant_prepare`, `dominant_custom_draw`, `dominant_slint_render`, or
`dominant_fb_present`.

## Benchmark

BEFORE command:

```bash
scripts/profile-preview-scroll.sh 30 turbo-hold ITEM04-BEFORE-slow-attribution-cpu --deploy-device --cpu-profile --visual-captures 0
```

BEFORE artifacts:

- `build/preview-scroll-profiles/ITEM04-BEFORE-slow-attribution-cpu-arcade.tsv`
- `build/preview-scroll-profiles/ITEM04-BEFORE-slow-attribution-cpu-arcade.log`
- `build/preview-scroll-profiles/ITEM04-BEFORE-slow-attribution-cpu-arcade.status.txt`
- `build/preview-scroll-profiles/ITEM04-BEFORE-slow-attribution-cpu-arcade-cpu.svg`

BEFORE metrics:

- Slow work frames: 5.
- Attributed slow work frames: 0.
- Unattributed slow work frames: 5.
- Detail columns present: no.

AFTER command:

```bash
scripts/profile-preview-scroll.sh 30 turbo-hold ITEM04-AFTER-slow-attribution-v2 --skip-build --cpu-profile --visual-captures 0
```

AFTER artifacts:

- `build/preview-scroll-profiles/ITEM04-AFTER-slow-attribution-v2-arcade.tsv`
- `build/preview-scroll-profiles/ITEM04-AFTER-slow-attribution-v2-arcade.log`
- `build/preview-scroll-profiles/ITEM04-AFTER-slow-attribution-v2-arcade.status.txt`
- `build/preview-scroll-profiles/ITEM04-AFTER-slow-attribution-v2-arcade-cpu.svg`

AFTER metrics:

- Slow work frames: 4.
- Attributed slow work frames: 4.
- Unattributed slow work frames: 0.
- Detail columns present: yes.
- Script metric row: `metric_tsv ... metric=unattributed_slow_work_frames value=0`.

## Validation

```bash
scripts/profile-preview-scroll.sh --self-test
cargo test --manifest-path magik-gui/Cargo.toml --features ui --no-default-features held_scroll_keeps_initial_press_when_summary_has_no_rows
scripts/test-host-tools.sh
cargo clippy --manifest-path magik-gui/Cargo.toml --lib --no-default-features -- -D warnings
cargo clippy --manifest-path tools/mister/Cargo.toml --all-targets -- -D warnings
```
